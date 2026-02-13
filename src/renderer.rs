use std::collections::HashMap;
use std::sync::Arc;

use cosmic_text::{
    Attrs, Buffer, Color as CosmicColor, Family, FontSystem, Metrics, Shaping, SwashCache,
};
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::config::{parse_hex_color, Config};
use crate::grid::PaneLayout;
use crate::terminal::{Cell, CellColor};

/// Data needed to render a single pane
pub struct PaneRenderData {
    pub title: String,
    pub lines: Vec<Vec<Cell>>,
    pub cursor: (usize, usize),
    pub is_focused: bool,
}

/// Vertex for textured quad rendering
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    tex_coords: [f32; 2],
    color: [f32; 4],
}

/// Vertex for solid-color rectangles
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct RectVertex {
    position: [f32; 2],
    color: [f32; 4],
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    width: u32,
    height: u32,

    // Text rendering
    font_system: FontSystem,
    swash_cache: SwashCache,
    font_size: f32,
    line_height: f32,

    // Glyph atlas
    atlas_texture: wgpu::Texture,
    atlas_view: wgpu::TextureView,
    atlas_sampler: wgpu::Sampler,
    atlas_size: u32,
    atlas_cursor_x: u32,
    atlas_cursor_y: u32,
    atlas_row_height: u32,
    glyph_cache: HashMap<GlyphKey, GlyphInfo>,

    // Pipelines
    rect_pipeline: wgpu::RenderPipeline,
    text_pipeline: wgpu::RenderPipeline,
    text_bind_group_layout: wgpu::BindGroupLayout,
    text_bind_group: wgpu::BindGroup,

    // Config colors
    bg_color: [f32; 4],
    fg_color: [f32; 4],
    cursor_color: [f32; 4],
    border_color: [f32; 4],
    border_focused_color: [f32; 4],
    title_bg_color: [f32; 4],
    title_fg_color: [f32; 4],
    ansi_colors: [[f32; 4]; 16],

    cell_width: f32,
    cell_height: f32,

    config: Arc<Config>,
}

#[derive(Hash, Eq, PartialEq, Clone)]
struct GlyphKey {
    ch: char,
    bold: bool,
    italic: bool,
}

#[derive(Clone)]
struct GlyphInfo {
    tex_x: f32,
    tex_y: f32,
    tex_w: f32,
    tex_h: f32,
    width: f32,
    height: f32,
    offset_x: f32,
    offset_y: f32,
}

impl Renderer {
    pub async fn new(window: Arc<Window>, config: &Config) -> Self {
        let size = window.inner_size();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let surface = instance.create_surface(window.clone()).unwrap();

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("Failed to find GPU adapter");

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Termania Device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            }, None)
            .await
            .expect("Failed to create device");

        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(surface_caps.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        // Font system
        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();

        let font_size = config.font.size;
        let line_height_mult = config.font.line_height;

        // Measure a character to get cell dimensions
        let metrics = Metrics::new(font_size, font_size * line_height_mult);
        let mut measure_buf = Buffer::new(&mut font_system, metrics);
        measure_buf.set_size(&mut font_system, Some(500.0), Some(100.0));
        measure_buf.set_text(
            &mut font_system,
            "M",
            Attrs::new().family(Family::Name(&config.font.family)),
            Shaping::Advanced,
        );
        measure_buf.shape_until_scroll(&mut font_system, false);

        // Approximate cell size
        let cell_width = font_size * 0.6;
        let cell_height = font_size * line_height_mult;

        // Create glyph atlas texture
        let atlas_size: u32 = 2048;
        let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Glyph Atlas"),
            size: wgpu::Extent3d {
                width: atlas_size,
                height: atlas_size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Shader for solid rectangles
        let rect_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rect Shader"),
            source: wgpu::ShaderSource::Wgsl(RECT_SHADER.into()),
        });

        let rect_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Rect Pipeline Layout"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });

        let rect_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Rect Pipeline"),
            layout: Some(&rect_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &rect_shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<RectVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 8,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x4,
                        },
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &rect_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Shader for textured glyphs
        let text_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Text Shader"),
            source: wgpu::ShaderSource::Wgsl(TEXT_SHADER.into()),
        });

        let text_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Text Bind Group Layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            multisampled: false,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let text_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Text Bind Group"),
            layout: &text_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&atlas_sampler),
                },
            ],
        });

        let text_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Text Pipeline Layout"),
                bind_group_layouts: &[&text_bind_group_layout],
                push_constant_ranges: &[],
            });

        let text_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Text Pipeline"),
            layout: Some(&text_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &text_shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 8,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 16,
                            shader_location: 2,
                            format: wgpu::VertexFormat::Float32x4,
                        },
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &text_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Parse colors
        let bg_color = parse_hex_color(&config.colors.background);
        let fg_color = parse_hex_color(&config.colors.foreground);
        let cursor_color = parse_hex_color(&config.colors.cursor);
        let border_color = parse_hex_color(&config.colors.border);
        let border_focused_color = parse_hex_color(&config.colors.border_focused);
        let title_bg_color = parse_hex_color(&config.colors.title_bg);
        let title_fg_color = parse_hex_color(&config.colors.title_fg);

        let mut ansi_colors = [[0.0f32; 4]; 16];
        for (i, hex) in config.colors.ansi.iter().enumerate() {
            ansi_colors[i] = parse_hex_color(hex);
        }

        Self {
            surface,
            device,
            queue,
            surface_config,
            width: size.width,
            height: size.height,
            font_system,
            swash_cache,
            font_size,
            line_height: cell_height,
            atlas_texture,
            atlas_view,
            atlas_sampler,
            atlas_size,
            atlas_cursor_x: 0,
            atlas_cursor_y: 0,
            atlas_row_height: 0,
            glyph_cache: HashMap::new(),
            rect_pipeline,
            text_pipeline,
            text_bind_group_layout,
            text_bind_group,
            bg_color,
            fg_color,
            cursor_color,
            border_color,
            border_focused_color,
            title_bg_color,
            title_fg_color,
            ansi_colors,
            cell_width,
            cell_height,
            config: Arc::new(config.clone()),
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.width = width;
            self.height = height;
            self.surface_config.width = width;
            self.surface_config.height = height;
            self.surface.configure(&self.device, &self.surface_config);
        }
    }

    pub fn adjust_font_size(&mut self, delta: f32) {
        let new_size = (self.font_size + delta).clamp(8.0, 72.0);
        self.set_font_size(new_size);
    }

    pub fn set_font_size(&mut self, size: f32) {
        self.font_size = size.clamp(8.0, 72.0);
        self.cell_width = self.font_size * 0.6;
        self.cell_height = self.font_size * self.config.font.line_height;
        self.line_height = self.cell_height;
        // Clear glyph cache since size changed
        self.glyph_cache.clear();
        self.atlas_cursor_x = 0;
        self.atlas_cursor_y = 0;
        self.atlas_row_height = 0;
    }

    /// Convert pixel coordinates to NDC
    fn to_ndc(&self, x: f32, y: f32) -> [f32; 2] {
        [
            (x / self.width as f32) * 2.0 - 1.0,
            1.0 - (y / self.height as f32) * 2.0,
        ]
    }

    fn resolve_color(&self, color: &CellColor, is_fg: bool) -> [f32; 4] {
        match color {
            CellColor::Default => {
                if is_fg {
                    self.fg_color
                } else {
                    self.bg_color
                }
            }
            CellColor::Ansi(idx) => {
                if (*idx as usize) < 16 {
                    self.ansi_colors[*idx as usize]
                } else {
                    self.fg_color
                }
            }
            CellColor::Rgb(r, g, b) => [*r as f32 / 255.0, *g as f32 / 255.0, *b as f32 / 255.0, 1.0],
            CellColor::Indexed(idx) => {
                if (*idx as usize) < 16 {
                    self.ansi_colors[*idx as usize]
                } else if *idx < 232 {
                    // 216-color cube
                    let idx = *idx - 16;
                    let r = (idx / 36) % 6;
                    let g = (idx / 6) % 6;
                    let b = idx % 6;
                    [
                        if r > 0 { (r as f32 * 40.0 + 55.0) / 255.0 } else { 0.0 },
                        if g > 0 { (g as f32 * 40.0 + 55.0) / 255.0 } else { 0.0 },
                        if b > 0 { (b as f32 * 40.0 + 55.0) / 255.0 } else { 0.0 },
                        1.0,
                    ]
                } else {
                    // Grayscale ramp
                    let v = ((*idx - 232) as f32 * 10.0 + 8.0) / 255.0;
                    [v, v, v, 1.0]
                }
            }
        }
    }

    fn rasterize_glyph(&mut self, ch: char, bold: bool, italic: bool) -> GlyphInfo {
        let key = GlyphKey { ch, bold, italic };
        if let Some(info) = self.glyph_cache.get(&key) {
            return info.clone();
        }

        // Render the glyph using cosmic-text
        let metrics = Metrics::new(self.font_size, self.line_height);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(&mut self.font_system, Some(self.font_size * 2.0), Some(self.line_height * 2.0));

        let mut attrs = Attrs::new().family(Family::Name(&self.config.font.family));
        if bold {
            attrs = attrs.weight(cosmic_text::Weight::BOLD);
        }
        if italic {
            attrs = attrs.style(cosmic_text::Style::Italic);
        }

        let s = ch.to_string();
        buffer.set_text(&mut self.font_system, &s, attrs, Shaping::Advanced);
        buffer.shape_until_scroll(&mut self.font_system, false);

        // Draw to get pixel data
        let mut glyph_pixels: Vec<u8> = Vec::new();
        let mut glyph_w: u32 = 0;
        let mut glyph_h: u32 = 0;
        let offset_x: f32 = 0.0;
        let offset_y: f32 = 0.0;

        // Use swash to rasterize
        let color = CosmicColor::rgb(255, 255, 255);
        buffer.draw(&mut self.font_system, &mut self.swash_cache, color, |x, y, w, h, c| {
            let alpha = ((c.0 >> 24) & 0xFF) as u8;
            // We just need the alpha channel for the glyph
            let pixel_w = w as u32;
            let pixel_h = h as u32;

            if pixel_w > 0 && pixel_h > 0 {
                // This is called per-glyph run, expand our buffer
                let new_w = (x as u32 + pixel_w).max(glyph_w);
                let new_h = (y as u32 + pixel_h).max(glyph_h);

                if new_w != glyph_w || new_h != glyph_h {
                    let mut new_pixels = vec![0u8; (new_w * new_h) as usize];
                    // Copy old data
                    for row in 0..glyph_h {
                        for col in 0..glyph_w {
                            if row < new_h && col < new_w {
                                new_pixels[(row * new_w + col) as usize] =
                                    glyph_pixels[(row * glyph_w + col) as usize];
                            }
                        }
                    }
                    glyph_pixels = new_pixels;
                    glyph_w = new_w;
                    glyph_h = new_h;
                }

                // Write the pixel
                let px = x as u32;
                let py = y as u32;
                if px < glyph_w && py < glyph_h {
                    glyph_pixels[(py * glyph_w + px) as usize] = alpha;
                }
            }
        });

        // If no glyph was rasterized, create a minimal placeholder
        if glyph_w == 0 || glyph_h == 0 {
            glyph_w = 1;
            glyph_h = 1;
            glyph_pixels = vec![0];
        }

        // Upload to atlas
        if self.atlas_cursor_x + glyph_w > self.atlas_size {
            self.atlas_cursor_x = 0;
            self.atlas_cursor_y += self.atlas_row_height + 1;
            self.atlas_row_height = 0;
        }

        if self.atlas_cursor_y + glyph_h > self.atlas_size {
            // Atlas full - reset (simple approach)
            self.atlas_cursor_x = 0;
            self.atlas_cursor_y = 0;
            self.atlas_row_height = 0;
            self.glyph_cache.clear();
        }

        let atlas_x = self.atlas_cursor_x;
        let atlas_y = self.atlas_cursor_y;

        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: atlas_x,
                    y: atlas_y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &glyph_pixels,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(glyph_w),
                rows_per_image: Some(glyph_h),
            },
            wgpu::Extent3d {
                width: glyph_w,
                height: glyph_h,
                depth_or_array_layers: 1,
            },
        );

        self.atlas_cursor_x += glyph_w + 1;
        self.atlas_row_height = self.atlas_row_height.max(glyph_h);

        let info = GlyphInfo {
            tex_x: atlas_x as f32 / self.atlas_size as f32,
            tex_y: atlas_y as f32 / self.atlas_size as f32,
            tex_w: glyph_w as f32 / self.atlas_size as f32,
            tex_h: glyph_h as f32 / self.atlas_size as f32,
            width: glyph_w as f32,
            height: glyph_h as f32,
            offset_x,
            offset_y,
        };

        self.glyph_cache.insert(key, info.clone());
        info
    }

    /// Push a solid-color rectangle
    fn push_rect(
        vertices: &mut Vec<RectVertex>,
        indices: &mut Vec<u32>,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        let base = vertices.len() as u32;

        let to_ndc = |px: f32, py: f32| -> [f32; 2] {
            [
                (px / screen_w) * 2.0 - 1.0,
                1.0 - (py / screen_h) * 2.0,
            ]
        };

        vertices.push(RectVertex { position: to_ndc(x, y), color });
        vertices.push(RectVertex { position: to_ndc(x + w, y), color });
        vertices.push(RectVertex { position: to_ndc(x + w, y + h), color });
        vertices.push(RectVertex { position: to_ndc(x, y + h), color });

        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// Push a textured glyph quad
    fn push_glyph(
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        x: f32,
        y: f32,
        glyph: &GlyphInfo,
        color: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) {
        let base = vertices.len() as u32;

        let to_ndc = |px: f32, py: f32| -> [f32; 2] {
            [
                (px / screen_w) * 2.0 - 1.0,
                1.0 - (py / screen_h) * 2.0,
            ]
        };

        let gx = x + glyph.offset_x;
        let gy = y + glyph.offset_y;

        vertices.push(Vertex {
            position: to_ndc(gx, gy),
            tex_coords: [glyph.tex_x, glyph.tex_y],
            color,
        });
        vertices.push(Vertex {
            position: to_ndc(gx + glyph.width, gy),
            tex_coords: [glyph.tex_x + glyph.tex_w, glyph.tex_y],
            color,
        });
        vertices.push(Vertex {
            position: to_ndc(gx + glyph.width, gy + glyph.height),
            tex_coords: [glyph.tex_x + glyph.tex_w, glyph.tex_y + glyph.tex_h],
            color,
        });
        vertices.push(Vertex {
            position: to_ndc(gx, gy + glyph.height),
            tex_coords: [glyph.tex_x, glyph.tex_y + glyph.tex_h],
            color,
        });

        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    pub fn render(&mut self, panes: &[PaneRenderData], layouts: &[PaneLayout]) {
        let output = match self.surface.get_current_texture() {
            Ok(t) => t,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.surface_config);
                return;
            }
            Err(e) => {
                log::error!("Surface error: {:?}", e);
                return;
            }
        };

        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Render Encoder"),
        });

        let sw = self.width as f32;
        let sh = self.height as f32;

        // Build geometry
        let mut rect_vertices: Vec<RectVertex> = Vec::new();
        let mut rect_indices: Vec<u32> = Vec::new();
        let mut text_vertices: Vec<Vertex> = Vec::new();
        let mut text_indices: Vec<u32> = Vec::new();

        let border_width = 2.0f32;

        for (i, layout) in layouts.iter().enumerate() {
            if i >= panes.len() {
                break;
            }
            let pane = &panes[i];

            // Pane border
            let border_color = if pane.is_focused {
                self.border_focused_color
            } else {
                self.border_color
            };

            // Top border
            Self::push_rect(&mut rect_vertices, &mut rect_indices,
                layout.x, layout.y, layout.width, border_width,
                border_color, sw, sh);
            // Bottom border
            Self::push_rect(&mut rect_vertices, &mut rect_indices,
                layout.x, layout.y + layout.height - border_width, layout.width, border_width,
                border_color, sw, sh);
            // Left border
            Self::push_rect(&mut rect_vertices, &mut rect_indices,
                layout.x, layout.y, border_width, layout.height,
                border_color, sw, sh);
            // Right border
            Self::push_rect(&mut rect_vertices, &mut rect_indices,
                layout.x + layout.width - border_width, layout.y, border_width, layout.height,
                border_color, sw, sh);

            // Title bar background
            let title_y = layout.y + border_width;
            Self::push_rect(&mut rect_vertices, &mut rect_indices,
                layout.x + border_width, title_y,
                layout.width - 2.0 * border_width, layout.title_height,
                self.title_bg_color, sw, sh);

            // Title text
            let title_chars: Vec<char> = pane.title.chars().collect();
            let title_x = layout.x + border_width + 8.0;
            let title_text_y = title_y + (layout.title_height - self.font_size) / 2.0;
            for (ci, ch) in title_chars.iter().enumerate() {
                let glyph = self.rasterize_glyph(*ch, true, false);
                let cx = title_x + ci as f32 * self.cell_width;
                Self::push_glyph(&mut text_vertices, &mut text_indices,
                    cx, title_text_y, &glyph, self.title_fg_color, sw, sh);
            }

            // Terminal content area
            let content_x = layout.x + border_width + self.config.grid.inner_padding as f32;
            let content_y = title_y + layout.title_height;
            let inner_pad = self.config.grid.inner_padding as f32;

            // Background for terminal area
            Self::push_rect(&mut rect_vertices, &mut rect_indices,
                layout.x + border_width, content_y,
                layout.width - 2.0 * border_width,
                layout.height - border_width * 2.0 - layout.title_height,
                self.bg_color, sw, sh);

            // Render terminal cells
            for (row_idx, row) in pane.lines.iter().enumerate() {
                for (col_idx, cell) in row.iter().enumerate() {
                    let cx = content_x + col_idx as f32 * self.cell_width;
                    let cy = content_y + inner_pad + row_idx as f32 * self.cell_height;

                    // Check bounds
                    if cx + self.cell_width > layout.x + layout.width - border_width {
                        break;
                    }
                    if cy + self.cell_height > layout.y + layout.height - border_width {
                        break;
                    }

                    let (fg_color, bg_color) = if cell.inverse {
                        (
                            self.resolve_color(&cell.bg, false),
                            self.resolve_color(&cell.fg, true),
                        )
                    } else {
                        (
                            self.resolve_color(&cell.fg, true),
                            self.resolve_color(&cell.bg, false),
                        )
                    };

                    // Draw background if not default
                    if cell.bg != CellColor::Default || cell.inverse {
                        Self::push_rect(&mut rect_vertices, &mut rect_indices,
                            cx, cy, self.cell_width, self.cell_height,
                            bg_color, sw, sh);
                    }

                    // Draw cursor
                    if row_idx == pane.cursor.0 && col_idx == pane.cursor.1 && pane.is_focused {
                        Self::push_rect(&mut rect_vertices, &mut rect_indices,
                            cx, cy, self.cell_width, self.cell_height,
                            self.cursor_color, sw, sh);
                    }

                    // Draw character
                    if cell.ch != ' ' && cell.ch != '\0' {
                        let glyph = self.rasterize_glyph(cell.ch, cell.bold, cell.italic);
                        let text_color = if row_idx == pane.cursor.0
                            && col_idx == pane.cursor.1
                            && pane.is_focused
                        {
                            self.bg_color
                        } else {
                            fg_color
                        };
                        Self::push_glyph(&mut text_vertices, &mut text_indices,
                            cx, cy, &glyph, text_color, sw, sh);
                    }

                    // Draw underline
                    if cell.underline {
                        Self::push_rect(&mut rect_vertices, &mut rect_indices,
                            cx, cy + self.cell_height - 2.0, self.cell_width, 1.0,
                            fg_color, sw, sh);
                    }
                }
            }
        }

        // Create GPU buffers
        let rect_vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rect VB"),
            contents: bytemuck::cast_slice(&rect_vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let rect_ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rect IB"),
            contents: bytemuck::cast_slice(&rect_indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let text_vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Text VB"),
            contents: bytemuck::cast_slice(&text_vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let text_ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Text IB"),
            contents: bytemuck::cast_slice(&text_indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        // Render pass
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Main Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: self.bg_color[0] as f64,
                            g: self.bg_color[1] as f64,
                            b: self.bg_color[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });

            // Draw rectangles
            if !rect_indices.is_empty() {
                pass.set_pipeline(&self.rect_pipeline);
                pass.set_vertex_buffer(0, rect_vb.slice(..));
                pass.set_index_buffer(rect_ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..rect_indices.len() as u32, 0, 0..1);
            }

            // Draw text
            if !text_indices.is_empty() {
                pass.set_pipeline(&self.text_pipeline);
                pass.set_bind_group(0, &self.text_bind_group, &[]);
                pass.set_vertex_buffer(0, text_vb.slice(..));
                pass.set_index_buffer(text_ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..text_indices.len() as u32, 0, 0..1);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
    }
}

/// WGSL shader for solid-color rectangles
const RECT_SHADER: &str = r#"
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4<f32>(in.position, 0.0, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

/// WGSL shader for textured glyph quads
const TEXT_SHADER: &str = r#"
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) tex_coords: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@group(0) @binding(0)
var t_glyph: texture_2d<f32>;
@group(0) @binding(1)
var s_glyph: sampler;

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4<f32>(in.position, 0.0, 1.0);
    out.tex_coords = in.tex_coords;
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let alpha = textureSample(t_glyph, s_glyph, in.tex_coords).r;
    return vec4<f32>(in.color.rgb, in.color.a * alpha);
}
"#;
