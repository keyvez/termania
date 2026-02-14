use std::collections::HashMap;
use std::sync::Arc;

use cosmic_text::{
    Attrs, Buffer, Color as CosmicColor, Family, FontSystem, Metrics, Shaping, SwashCache,
};
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::config::{parse_hex_color, Config};
use crate::grid::PaneLayout;
use crate::plugin::PanePluginRenderData;
use crate::terminal::CellColor;

/// Data needed to render a single pane
pub struct PaneRenderData {
    pub title: String,
    pub plugin_data: PanePluginRenderData,
    pub is_focused: bool,
    pub watermark: Option<String>,
    /// Whether this pane has detected an error (for red tint overlay)
    pub has_error: bool,
    /// Whether this pane is selected (for multi-select highlight)
    pub is_selected: bool,
    /// Whether broadcast mode is active
    pub broadcast_mode: bool,
    /// If set, this pane's title bar shows a rename input field
    pub rename_input: Option<String>,
}

/// Data for the command overlay
pub struct OverlayRenderData {
    pub text: String,
    pub target_label: String,
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
    /// Logical font size in points (user-facing, e.g. 14pt)
    font_size: f32,
    /// Physical font size = font_size * scale_factor (used for rasterization)
    physical_font_size: f32,
    line_height: f32,
    /// Display scale factor (2.0 on Retina)
    scale_factor: f32,

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

    /// Cell width in physical pixels
    cell_width: f32,
    /// Cell height in physical pixels
    cell_height: f32,
    /// Font advance as fraction of font size (advance / upem). Used to compute
    /// cell widths at any font size for centering glyphs.
    advance_ratio: f32,

    config: Arc<Config>,
    /// Resolved font family name (may differ from config, e.g. "SF Mono" -> ".SF NS Mono")
    resolved_font_family: String,
    /// Actual font weight for normal text (may differ from Weight::NORMAL if font only has Light)
    font_weight_normal: cosmic_text::Weight,
    /// Actual font weight for bold text (closest bold available, or same as normal if none)
    font_weight_bold: cosmic_text::Weight,
    /// Path to save a screenshot on next render, if set
    pending_screenshot: Option<String>,
}

#[derive(Hash, Eq, PartialEq, Clone)]
struct GlyphKey {
    ch: char,
    bold: bool,
    italic: bool,
    /// Font size in centi-points (to distinguish watermark glyphs from regular)
    size_cp: u32,
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
    /// Resolve a font family name to one that fontdb/cosmic-text can find.
    ///
    /// macOS system fonts like "SF Mono" are registered under different names
    /// in fontdb (e.g. ".SF NS Mono"). This function searches fontdb directly
    /// for matching font faces.
    fn resolve_font_family(font_system: &mut FontSystem, family: &str) -> String {
        // Direct search in fontdb for exact family name match
        for face in font_system.db().faces() {
            for (fam_name, _) in &face.families {
                if fam_name.eq_ignore_ascii_case(family) {
                    log::info!("Font '{}' found as '{}'", family, fam_name);
                    return fam_name.clone();
                }
            }
        }

        // macOS font name aliases to try
        let aliases: &[(&str, &[&str])] = &[
            ("SF Mono", &[".SF NS Mono"]),
            ("SF Pro", &[".SF NS"]),
            ("SF Compact", &[".SF Compact"]),
            ("New York", &[".New York"]),
        ];

        for (name, candidates) in aliases {
            if family.eq_ignore_ascii_case(name) {
                for candidate in *candidates {
                    for face in font_system.db().faces() {
                        for (fam_name, _) in &face.families {
                            if fam_name == candidate {
                                log::info!("Font '{}' resolved via alias to '{}'", family, fam_name);
                                return fam_name.clone();
                            }
                        }
                    }
                }
            }
        }

        // Fuzzy match: look for monospace fonts whose name contains key words from the query
        let family_words: Vec<&str> = family.split_whitespace().collect();
        if family_words.len() >= 2 {
            for face in font_system.db().faces() {
                for (fam_name, _) in &face.families {
                    let lower = fam_name.to_lowercase();
                    if family_words.iter().all(|w| lower.contains(&w.to_lowercase())) {
                        log::info!("Font '{}' resolved to '{}' (fuzzy match)", family, fam_name);
                        return fam_name.clone();
                    }
                }
            }
        }

        log::warn!("Font '{}' not found in fontdb, using as-is (will fall back to system default)", family);
        family.to_string()
    }

    /// Measure cell dimensions by using cosmic-text's layout to determine
    /// the font's advance width at the given size.
    /// Returns (cell_width, cell_height, advance_ratio) where advance_ratio
    /// is advance/font_size so cell_width at any font size = size * advance_ratio.
    fn measure_cell_size(
        font_system: &mut FontSystem,
        phys_font_size: f32,
        line_height_mult: f32,
        font_family: &str,
    ) -> (f32, f32, f32) {
        let line_h = phys_font_size * line_height_mult;

        // Find the font's advance width directly from fontdb face metrics.
        // cosmic-text's Family::Name() lookup may not match macOS system fonts
        // (e.g. ".SF NS Mono"), so we query fontdb directly for the font face
        // and read its advance width from the font tables via ttf-parser.
        let mut cell_w = phys_font_size * 0.6; // fallback

        // Find the font face ID and use with_face_data to access the font data
        // (handles all Source variants including SharedFile/memmap)
        // Prefer Regular weight over Light for accurate cell width
        let mut found_id = None;
        let mut found_light_id = None;
        for face_info in font_system.db().faces() {
            let matches = face_info.families.iter().any(|(name, _)| {
                name.eq_ignore_ascii_case(font_family)
            });
            if matches && face_info.style == cosmic_text::Style::Normal {
                let is_regular = face_info.weight == cosmic_text::Weight::NORMAL
                    || face_info.post_script_name.contains("Regular");
                if is_regular {
                    log::info!("measure_cell_size: matched face '{}' (Regular) id={:?}", face_info.post_script_name, face_info.id);
                    found_id = Some(face_info.id);
                    break;
                } else if found_light_id.is_none() {
                    found_light_id = Some((face_info.id, face_info.post_script_name.clone()));
                }
            }
        }
        if found_id.is_none() {
            if let Some((id, name)) = found_light_id {
                log::info!("measure_cell_size: no Regular found, using '{}' id={:?}", name, id);
                found_id = Some(id);
            }
        }

        if let Some(face_id) = found_id {
            if let Some(advance_w) = font_system.db().with_face_data(face_id, |data, index| {
                if let Ok(face) = ttf_parser::Face::parse(data, index) {
                    let upem = face.units_per_em() as f32;
                    if let Some(gid) = face.glyph_index('M') {
                        if let Some(advance) = face.glyph_hor_advance(gid) {
                            let w = (advance as f32 / upem) * phys_font_size;
                            log::info!("measure_cell_size: font='{}', advance={}, upem={}, cell_w={:.1}, font_size={:.1}",
                                font_family, advance, upem as u32, w, phys_font_size);
                            return Some(w);
                        }
                    }
                }
                None
            }) {
                if let Some(w) = advance_w {
                    cell_w = w;
                    let ratio = cell_w / phys_font_size;
                    return (cell_w, line_h, ratio);
                }
            }
        }

        let ratio = cell_w / phys_font_size;
        log::warn!("measure_cell_size: font '{}' not found in fontdb, using fallback cell_w={:.1}", font_family, cell_w);
        (cell_w, line_h, ratio)
    }

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
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
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

        let scale_factor = window.scale_factor() as f32;
        let font_size = config.font.size;
        let physical_font_size = font_size * scale_factor;
        let line_height_mult = config.font.line_height;

        // Resolve the font family name (handles macOS system font aliases)
        let resolved_font_family = Self::resolve_font_family(&mut font_system, &config.font.family);
        log::info!("Font family: '{}' -> '{}'", config.font.family, resolved_font_family);

        // Discover available font weights for this font family.
        // macOS system fonts (e.g. .SF NS Mono) may only expose Light (295) weight
        // via fontdb, not Regular (400) or Bold (700). We need to use the actual
        // available weight so cosmic-text doesn't fall back to different fonts.
        let mut font_weight_normal = cosmic_text::Weight::NORMAL;
        let mut font_weight_bold = cosmic_text::Weight::BOLD;
        let mut available_weights: Vec<cosmic_text::Weight> = Vec::new();
        for face_info in font_system.db().faces() {
            let matches_family = face_info.families.iter().any(|(name, _)| {
                name.eq_ignore_ascii_case(&resolved_font_family)
            });
            if matches_family && face_info.style == cosmic_text::Style::Normal {
                log::info!("  Available face: '{}' weight={:?}", face_info.post_script_name, face_info.weight);
                available_weights.push(face_info.weight);
            }
        }
        if !available_weights.is_empty() {
            // For normal text, pick the weight closest to 400 (Regular)
            available_weights.sort_by_key(|w| (w.0 as i32 - 400).unsigned_abs());
            font_weight_normal = available_weights[0];
            // For bold text, pick the weight closest to 700 (Bold)
            available_weights.sort_by_key(|w| (w.0 as i32 - 700).unsigned_abs());
            font_weight_bold = available_weights[0];
            log::info!("Font weights: normal={:?}, bold={:?}", font_weight_normal, font_weight_bold);
        }

        // Measure cell dimensions at physical font size for pixel-accurate HiDPI rendering.
        // All rasterization and layout uses physical pixels; the wgpu surface is physical.
        let (cell_width, cell_height, advance_ratio) = Self::measure_cell_size(
            &mut font_system, physical_font_size, line_height_mult, &resolved_font_family,
        );
        log::debug!("Font metrics: size={}pt (physical={}pt), cell={}x{}px, scale={}",
            font_size, physical_font_size, cell_width, cell_height, scale_factor);

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
            physical_font_size,
            line_height: cell_height,
            scale_factor,
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
            advance_ratio,
            config: Arc::new(config.clone()),
            resolved_font_family,
            font_weight_normal,
            font_weight_bold,
            pending_screenshot: std::env::var("TERMANIA_SCREENSHOT").ok(),
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn cell_width(&self) -> f32 {
        self.cell_width
    }

    pub fn cell_height(&self) -> f32 {
        self.cell_height
    }

    pub fn scale_factor(&self) -> f32 {
        self.scale_factor
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
        self.physical_font_size = self.font_size * self.scale_factor;
        let (cw, ch, ar) = Self::measure_cell_size(
            &mut self.font_system,
            self.physical_font_size,
            self.config.font.line_height,
            &self.resolved_font_family,
        );
        self.cell_width = cw;
        self.cell_height = ch;
        self.line_height = ch;
        self.advance_ratio = ar;
        log::debug!("Font resize: logical={}pt, physical={}pt, cell={}x{}px, scale={}",
            self.font_size, self.physical_font_size, cw, ch, self.scale_factor);

        // Clear glyph cache and zero out the atlas texture to prevent stale data
        self.glyph_cache.clear();
        self.atlas_cursor_x = 0;
        self.atlas_cursor_y = 0;
        self.atlas_row_height = 0;
        // Clear the atlas texture with zeroes
        let zero_data = vec![0u8; (self.atlas_size * self.atlas_size) as usize];
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &zero_data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(self.atlas_size),
                rows_per_image: Some(self.atlas_size),
            },
            wgpu::Extent3d {
                width: self.atlas_size,
                height: self.atlas_size,
                depth_or_array_layers: 1,
            },
        );
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
        let key = GlyphKey { ch, bold, italic, size_cp: (self.physical_font_size * 100.0) as u32 };
        if let Some(info) = self.glyph_cache.get(&key) {
            return info.clone();
        }

        // Render the glyph at physical font size for pixel-accurate HiDPI rendering
        let metrics = Metrics::new(self.physical_font_size, self.line_height);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(&mut self.font_system, Some(self.physical_font_size * 2.0), Some(self.line_height * 2.0));

        let weight = if bold { self.font_weight_bold } else { self.font_weight_normal };
        let mut attrs = Attrs::new()
            .family(Family::Name(&self.resolved_font_family))
            .weight(weight);
        if italic {
            attrs = attrs.style(cosmic_text::Style::Italic);
        }

        let s = ch.to_string();
        buffer.set_text(&mut self.font_system, &s, attrs, Shaping::Advanced);
        buffer.shape_until_scroll(&mut self.font_system, false);

        // Log when cosmic-text uses a different font family (fallback) for this glyph
        for run in buffer.layout_runs() {
            for g in run.glyphs.iter() {
                if let Some(face) = self.font_system.db().face(g.font_id) {
                    let is_expected = face.families.iter().any(|(name, _)| {
                        name.eq_ignore_ascii_case(&self.resolved_font_family)
                    });
                    if !is_expected {
                        log::warn!("Glyph '{}': cosmic-text used font '{}' instead of {}",
                            ch, face.post_script_name, self.resolved_font_family);
                    }
                }
                break;
            }
            break;
        }

        // First pass: collect all pixel data with their positions to find ink bounds
        let mut pixels: Vec<(i32, i32, u8)> = Vec::new();
        let mut min_x: i32 = i32::MAX;
        let mut min_y: i32 = i32::MAX;
        let mut max_x: i32 = i32::MIN;
        let mut max_y: i32 = i32::MIN;

        let color = CosmicColor::rgb(255, 255, 255);
        buffer.draw(&mut self.font_system, &mut self.swash_cache, color, |x, y, _w, _h, c| {
            let alpha = ((c.0 >> 24) & 0xFF) as u8;
            if alpha > 0 && x >= 0 && y >= 0 {
                if x < min_x { min_x = x; }
                if y < min_y { min_y = y; }
                if x > max_x { max_x = x; }
                if y > max_y { max_y = y; }
                pixels.push((x, y, alpha));
            }
        });

        // Build a tight bitmap around the ink bounds and compute positioning
        let (glyph_w, glyph_h, glyph_pixels, offset_x, offset_y);
        if pixels.is_empty() || min_x > max_x {
            glyph_w = 1;
            glyph_h = 1;
            glyph_pixels = vec![0u8];
            offset_x = 0.0f32;
            offset_y = 0.0f32;
        } else {
            glyph_w = (max_x - min_x + 1) as u32;
            glyph_h = (max_y - min_y + 1) as u32;
            let mut buf = vec![0u8; (glyph_w * glyph_h) as usize];
            for (px, py, alpha) in &pixels {
                let lx = (px - min_x) as u32;
                let ly = (py - min_y) as u32;
                if lx < glyph_w && ly < glyph_h {
                    buf[(ly * glyph_w + lx) as usize] = *alpha;
                }
            }
            glyph_pixels = buf;
            // Center glyph within the cell. For monospace fonts, the glyph ink
            // is narrower than the cell advance, so centering distributes the
            // side bearing space evenly on both sides for consistent appearance.
            offset_x = (self.cell_width - glyph_w as f32) / 2.0;
            offset_y = min_y as f32;
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

    /// Rasterize a glyph at a custom font size (for watermarks). Cached.
    fn rasterize_glyph_at_size(&mut self, ch: char, size: f32) -> GlyphInfo {
        let key = GlyphKey { ch, bold: true, italic: false, size_cp: (size * 100.0) as u32 };
        if let Some(info) = self.glyph_cache.get(&key) {
            return info.clone();
        }

        let line_h = size * self.config.font.line_height;
        let metrics = Metrics::new(size, line_h);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(&mut self.font_system, Some(size * 2.0), Some(line_h * 2.0));

        let attrs = Attrs::new().family(Family::Name(&self.resolved_font_family))
            .weight(self.font_weight_bold);

        let s = ch.to_string();
        buffer.set_text(&mut self.font_system, &s, attrs, Shaping::Advanced);
        buffer.shape_until_scroll(&mut self.font_system, false);

        // Collect pixel data with positions to find tight ink bounds
        let mut pixels: Vec<(i32, i32, u8)> = Vec::new();
        let mut min_x: i32 = i32::MAX;
        let mut min_y: i32 = i32::MAX;
        let mut max_x: i32 = i32::MIN;
        let mut max_y: i32 = i32::MIN;

        let color = CosmicColor::rgb(255, 255, 255);
        buffer.draw(&mut self.font_system, &mut self.swash_cache, color, |x, y, _w, _h, c| {
            let alpha = ((c.0 >> 24) & 0xFF) as u8;
            if alpha > 0 && x >= 0 && y >= 0 {
                if x < min_x { min_x = x; }
                if y < min_y { min_y = y; }
                if x > max_x { max_x = x; }
                if y > max_y { max_y = y; }
                pixels.push((x, y, alpha));
            }
        });

        let (glyph_w, glyph_h, glyph_pixels, offset_x, offset_y);
        if pixels.is_empty() || min_x > max_x {
            glyph_w = 1;
            glyph_h = 1;
            glyph_pixels = vec![0u8];
            offset_x = 0.0f32;
            offset_y = 0.0f32;
        } else {
            glyph_w = (max_x - min_x + 1) as u32;
            glyph_h = (max_y - min_y + 1) as u32;
            let mut buf = vec![0u8; (glyph_w * glyph_h) as usize];
            for (px, py, alpha) in &pixels {
                let lx = (px - min_x) as u32;
                let ly = (py - min_y) as u32;
                if lx < glyph_w && ly < glyph_h {
                    buf[(ly * glyph_w + lx) as usize] = *alpha;
                }
            }
            glyph_pixels = buf;
            // Center horizontally within the cell for this font size
            let at_size_cell_w = size * self.advance_ratio;
            offset_x = (at_size_cell_w - glyph_w as f32) / 2.0;
            offset_y = min_y as f32;
        }

        // Upload to atlas
        if self.atlas_cursor_x + glyph_w > self.atlas_size {
            self.atlas_cursor_x = 0;
            self.atlas_cursor_y += self.atlas_row_height + 1;
            self.atlas_row_height = 0;
        }

        if self.atlas_cursor_y + glyph_h > self.atlas_size {
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
                origin: wgpu::Origin3d { x: atlas_x, y: atlas_y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &glyph_pixels,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(glyph_w),
                rows_per_image: Some(glyph_h),
            },
            wgpu::Extent3d { width: glyph_w, height: glyph_h, depth_or_array_layers: 1 },
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

    fn render_help_panel(
        &mut self,
        rect_vertices: &mut Vec<RectVertex>,
        rect_indices: &mut Vec<u32>,
        text_vertices: &mut Vec<Vertex>,
        text_indices: &mut Vec<u32>,
        sw: f32,
        sh: f32,
        scroll: usize,
    ) {
        let help_lines: &[(&str, &str)] = &[
            ("PANE NAVIGATION", ""),
            ("\u{2318}+1..9", "Jump to pane"),
            ("\u{2318}+]", "Next pane"),
            ("\u{2318}+[", "Previous pane"),
            ("Click", "Focus pane"),
            ("", ""),
            ("PANE MANAGEMENT", ""),
            ("\u{2318}+N", "New pane (in row)"),
            ("\u{2318}+\u{2325}+N", "New row"),
            ("\u{2318}+\u{21e7}+N", "New window"),
            ("\u{2318}+W", "Close pane"),
            ("\u{2318}+R", "Rename pane"),
            ("\u{2318}+\u{21e7}+\u{2190}", "Move pane left"),
            ("\u{2318}+\u{21e7}+\u{2192}", "Move pane right"),
            ("\u{2318}+\u{21e7}+\u{2191}", "Move pane up"),
            ("\u{2318}+\u{21e7}+\u{2193}", "Move pane down"),
            ("", ""),
            ("FONT", ""),
            ("\u{2318}+=", "Increase font size"),
            ("\u{2318}+-", "Decrease font size"),
            ("\u{2318}+0", "Reset font size"),
            ("", ""),
            ("BROADCAST & MULTI-SELECT", ""),
            ("\u{2318}+\u{21e7}+B", "Toggle broadcast mode"),
            ("\u{21e7}+Click+Drag", "Rectangle-select panes"),
            ("\u{2318}+Click", "Toggle pane selection"),
            ("\u{2318}+\u{21e7}+A", "Select all panes"),
            ("\u{2318}+\u{21e7}+D", "Deselect all"),
            ("", ""),
            ("COMMAND OVERLAY", ""),
            ("\u{2318}+\u{21e7}+\u{23ce}", "Open command overlay"),
            ("\u{2325}+\u{2325}", "Open command overlay"),
            ("Enter", "Send command"),
            ("Escape", "Cancel"),
            ("", ""),
            ("OTHER", ""),
            ("\u{2318}+,", "Open config file"),
            ("\u{2318}+/", "Toggle this help"),
        ];

        let help_font_size = self.physical_font_size * 1.2;
        let (help_cell_w, help_line_h, _) = Self::measure_cell_size(
            &mut self.font_system, help_font_size,
            self.config.font.line_height, &self.resolved_font_family,
        );

        let pad = 60.0 * self.scale_factor;
        let key_col_w = help_cell_w * 16.0;
        let panel_w = (key_col_w + help_cell_w * 26.0 + pad * 2.0).min(sw - 40.0 * self.scale_factor);
        let panel_x = (sw - panel_w) / 2.0;

        // Fixed header and footer heights
        let header_h = help_line_h * 2.0; // title + gap
        let footer_h = help_line_h * 1.5; // hint line

        // Full-screen solid background
        Self::push_rect(rect_vertices, rect_indices,
            0.0, 0.0, sw, sh,
            [0.08, 0.09, 0.11, 1.0], sw, sh);

        // -- Fixed header: title --
        let header_y = pad;
        let version_title = format!("Termania v{} - Keyboard Shortcuts", env!("CARGO_PKG_VERSION"));
        let title = &version_title;
        let title_color = [0.9, 0.9, 1.0, 1.0];
        for (ci, ch) in title.chars().enumerate() {
            let glyph = self.rasterize_glyph_at_size(ch, help_font_size);
            let cx = panel_x + pad + ci as f32 * help_cell_w;
            if cx + help_cell_w < panel_x + panel_w - pad {
                Self::push_glyph(text_vertices, text_indices,
                    cx, header_y, &glyph, title_color, sw, sh);
            }
        }

        // -- Scrollable content area --
        let content_top = pad + header_h;
        let content_bottom = sh - pad - footer_h;

        // Clamp scroll so we don't scroll past the end
        let total_lines = help_lines.len();
        let visible_lines = ((content_bottom - content_top) / help_line_h).floor().max(1.0) as usize;
        let max_scroll = total_lines.saturating_sub(visible_lines);
        let scroll = scroll.min(max_scroll);

        for (i, (key, desc)) in help_lines.iter().enumerate() {
            if i < scroll {
                continue;
            }
            let row_in_view = i - scroll;
            let y = content_top + row_in_view as f32 * help_line_h;

            // Stop if below the content area
            if y + help_line_h > content_bottom {
                break;
            }

            if key.is_empty() && desc.is_empty() {
                continue;
            }

            // Section headers (no desc)
            if !key.is_empty() && desc.is_empty() {
                let color = [0.5, 0.7, 1.0, 0.9];
                for (ci, ch) in key.chars().enumerate() {
                    let glyph = self.rasterize_glyph_at_size(ch, help_font_size);
                    let cx = panel_x + pad + ci as f32 * help_cell_w;
                    if cx + help_cell_w < panel_x + panel_w - pad {
                        Self::push_glyph(text_vertices, text_indices,
                            cx, y, &glyph, color, sw, sh);
                    }
                }
                continue;
            }

            // Key column
            let key_color = [0.9, 0.85, 0.6, 1.0];
            for (ci, ch) in key.chars().enumerate() {
                let glyph = self.rasterize_glyph_at_size(ch, help_font_size);
                let cx = panel_x + pad + ci as f32 * help_cell_w;
                if cx + help_cell_w < panel_x + pad + key_col_w {
                    Self::push_glyph(text_vertices, text_indices,
                        cx, y, &glyph, key_color, sw, sh);
                }
            }

            // Description column
            let desc_color = [0.75, 0.78, 0.82, 1.0];
            for (ci, ch) in desc.chars().enumerate() {
                let glyph = self.rasterize_glyph_at_size(ch, help_font_size);
                let cx = panel_x + pad + key_col_w + ci as f32 * help_cell_w;
                if cx + help_cell_w < panel_x + panel_w - pad {
                    Self::push_glyph(text_vertices, text_indices,
                        cx, y, &glyph, desc_color, sw, sh);
                }
            }
        }

        // -- Fixed footer: hints + scroll indicator --
        let footer_y = sh - pad - footer_h + help_line_h * 0.25;

        // Dismiss hint (left-aligned)
        let hint = "\u{238b} Escape to close   \u{2191}\u{2193} Scroll";
        let hint_color = [0.5, 0.5, 0.6, 0.8];
        for (ci, ch) in hint.chars().enumerate() {
            let glyph = self.rasterize_glyph_at_size(ch, help_font_size);
            let cx = panel_x + pad + ci as f32 * help_cell_w;
            if cx + help_cell_w < panel_x + panel_w - pad {
                Self::push_glyph(text_vertices, text_indices,
                    cx, footer_y, &glyph, hint_color, sw, sh);
            }
        }

        // Scroll position indicator (right-aligned)
        if max_scroll > 0 {
            let indicator = format!("{}/{}", scroll + 1, max_scroll + 1);
            let ind_w = indicator.len() as f32 * help_cell_w;
            let ind_x = panel_x + panel_w - pad - ind_w;
            for (ci, ch) in indicator.chars().enumerate() {
                let glyph = self.rasterize_glyph_at_size(ch, help_font_size);
                let cx = ind_x + ci as f32 * help_cell_w;
                Self::push_glyph(text_vertices, text_indices,
                    cx, footer_y, &glyph, hint_color, sw, sh);
            }
        }
    }

    pub fn render(&mut self, panes: &[PaneRenderData], layouts: &[PaneLayout], overlay: Option<&OverlayRenderData>, show_help: bool, help_scroll: usize) {
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
        let mut rect_vertices: Vec<RectVertex> = Vec::with_capacity(4096);
        let mut rect_indices: Vec<u32> = Vec::with_capacity(6144);
        let mut text_vertices: Vec<Vertex> = Vec::with_capacity(16384);
        let mut text_indices: Vec<u32> = Vec::with_capacity(24576);

        let border_width = 2.0f32 * self.scale_factor;

        for (i, layout) in layouts.iter().enumerate() {
            if i >= panes.len() {
                break;
            }
            let pane = &panes[i];

            // Pane border
            let border_color = if pane.is_selected {
                // Selected panes get a distinct color (orange/amber)
                [1.0, 0.7, 0.2, 1.0]
            } else if pane.is_focused {
                self.border_focused_color
            } else if pane.broadcast_mode {
                // Broadcast mode: all panes get a subtle green border
                [0.2, 0.8, 0.3, 0.7]
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

            // Title text (or rename input)
            let title_x = layout.x + border_width + 8.0 * self.scale_factor;
            let title_text_y = title_y + (layout.title_height - self.cell_height) / 2.0;

            if let Some(ref input) = pane.rename_input {
                // Rename mode: draw input field background
                let input_bg_w = layout.width - 2.0 * border_width - 16.0 * self.scale_factor;
                Self::push_rect(&mut rect_vertices, &mut rect_indices,
                    title_x - 4.0 * self.scale_factor, title_y + 2.0 * self.scale_factor,
                    input_bg_w, layout.title_height - 4.0 * self.scale_factor,
                    [0.15, 0.15, 0.2, 1.0], sw, sh);

                // Draw input text
                let display = format!("{}\u{2588}", input); // block cursor
                let input_color = [1.0, 1.0, 1.0, 1.0];
                for (ci, ch) in display.chars().enumerate() {
                    let glyph = self.rasterize_glyph(ch, true, false);
                    let cx = title_x + ci as f32 * self.cell_width;
                    if cx + self.cell_width < title_x + input_bg_w {
                        Self::push_glyph(&mut text_vertices, &mut text_indices,
                            cx, title_text_y, &glyph, input_color, sw, sh);
                    }
                }
            } else {
                // Normal title
                let title_chars: Vec<char> = pane.title.chars().collect();
                for (ci, ch) in title_chars.iter().enumerate() {
                    let glyph = self.rasterize_glyph(*ch, true, false);
                    let cx = title_x + ci as f32 * self.cell_width;
                    Self::push_glyph(&mut text_vertices, &mut text_indices,
                        cx, title_text_y, &glyph, self.title_fg_color, sw, sh);
                }
            }

            // Broadcast mode indicator in title bar
            if pane.broadcast_mode {
                let bc_text = "BC";
                let bc_x = layout.x + layout.width - border_width - 8.0 * self.scale_factor - bc_text.len() as f32 * self.cell_width;
                let bc_color = [0.2, 0.9, 0.3, 1.0]; // green
                for (ci, ch) in bc_text.chars().enumerate() {
                    let glyph = self.rasterize_glyph(ch, true, false);
                    let cx = bc_x + ci as f32 * self.cell_width;
                    Self::push_glyph(&mut text_vertices, &mut text_indices,
                        cx, title_text_y, &glyph, bc_color, sw, sh);
                }
            }

            // Selection indicator
            if pane.is_selected {
                let sel_color = [1.0, 0.7, 0.2, 1.0]; // amber
                let sel_indicator = "\u{2713}"; // checkmark
                let sel_x = layout.x + layout.width - border_width - 8.0 * self.scale_factor
                    - if pane.broadcast_mode { 3.0 * self.cell_width + 8.0 * self.scale_factor } else { 0.0 }
                    - sel_indicator.len() as f32 * self.cell_width;
                for (ci, ch) in sel_indicator.chars().enumerate() {
                    let glyph = self.rasterize_glyph(ch, true, false);
                    let cx = sel_x + ci as f32 * self.cell_width;
                    Self::push_glyph(&mut text_vertices, &mut text_indices,
                        cx, title_text_y, &glyph, sel_color, sw, sh);
                }
            }

            // Content area coordinates
            let inner_pad = self.config.grid.inner_padding as f32 * self.scale_factor;
            let content_x = layout.x + border_width + inner_pad;
            let content_y = title_y + layout.title_height;

            // Background for content area
            Self::push_rect(&mut rect_vertices, &mut rect_indices,
                layout.x + border_width, content_y,
                layout.width - 2.0 * border_width,
                layout.height - border_width * 2.0 - layout.title_height,
                self.bg_color, sw, sh);

            // Render content based on plugin type
            match &pane.plugin_data {
                PanePluginRenderData::Terminal { lines, cursor, watermark: term_watermark } => {
                    // Use watermark from render_data or from pane-level config
                    let watermark = pane.watermark.as_ref().or(term_watermark.as_ref());

                    // Render watermark text (large faded text behind content)
                    if let Some(watermark) = watermark {
                        let watermark_font_size = self.cell_height * 5.0;
                        let (watermark_char_w, _, _) = Self::measure_cell_size(
                            &mut self.font_system, watermark_font_size,
                            self.config.font.line_height, &self.resolved_font_family,
                        );
                        let watermark_total_w = watermark.len() as f32 * watermark_char_w;
                        let content_area_w = layout.width - 2.0 * border_width - 2.0 * inner_pad;
                        let content_area_h = layout.height - 2.0 * border_width - layout.title_height - 2.0 * inner_pad;
                        let wm_x = content_x + (content_area_w - watermark_total_w) / 2.0;
                        let wm_y = content_y + inner_pad + (content_area_h - watermark_font_size) / 2.0;
                        let watermark_color = [self.fg_color[0], self.fg_color[1], self.fg_color[2], 0.07];

                        for (ci, ch) in watermark.chars().enumerate() {
                            let glyph = self.rasterize_glyph_at_size(ch, watermark_font_size);
                            let cx = wm_x + ci as f32 * watermark_char_w;
                            Self::push_glyph(&mut text_vertices, &mut text_indices,
                                cx, wm_y, &glyph, watermark_color, sw, sh);
                        }
                    }

                    // Render terminal cells
                    for (row_idx, row) in lines.iter().enumerate() {
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
                            if row_idx == cursor.0 && col_idx == cursor.1 && pane.is_focused {
                                Self::push_rect(&mut rect_vertices, &mut rect_indices,
                                    cx, cy, self.cell_width, self.cell_height,
                                    self.cursor_color, sw, sh);
                            }

                            // Draw character
                            if cell.ch != ' ' && cell.ch != '\0' {
                                let glyph = self.rasterize_glyph(cell.ch, cell.bold, cell.italic);
                                let text_color = if row_idx == cursor.0
                                    && col_idx == cursor.1
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
                                    cx, cy + self.cell_height - 2.0 * self.scale_factor, self.cell_width, self.scale_factor,
                                    fg_color, sw, sh);
                            }
                        }
                    }
                }
                PanePluginRenderData::NativeView { .. } => {
                    // Native view is positioned as a subview — skip cell rendering.
                    // The NSView sits on top of the wgpu surface in this area.
                }
                PanePluginRenderData::GpuTexture { .. } => {
                    // TODO: Draw textured quad via image pipeline for screen capture frames.
                    // For now, show a placeholder message.
                    let placeholder = "Screen Capture";
                    let ph_color = [self.fg_color[0], self.fg_color[1], self.fg_color[2], 0.3];
                    let ph_x = content_x + inner_pad;
                    let ph_y = content_y + inner_pad;
                    for (ci, ch) in placeholder.chars().enumerate() {
                        let glyph = self.rasterize_glyph(ch, false, true);
                        let cx = ph_x + ci as f32 * self.cell_width;
                        Self::push_glyph(&mut text_vertices, &mut text_indices,
                            cx, ph_y, &glyph, ph_color, sw, sh);
                    }
                }
            }

            // Inactive pane fade overlay — dim unfocused panes
            if !pane.is_focused {
                Self::push_rect(&mut rect_vertices, &mut rect_indices,
                    layout.x + border_width, content_y,
                    layout.width - 2.0 * border_width,
                    layout.height - border_width * 2.0 - layout.title_height,
                    [0.0, 0.0, 0.0, 0.3], sw, sh);
            }

            // Error detection red tint overlay
            if pane.has_error {
                Self::push_rect(&mut rect_vertices, &mut rect_indices,
                    layout.x + border_width, content_y,
                    layout.width - 2.0 * border_width,
                    layout.height - border_width * 2.0 - layout.title_height,
                    [0.8, 0.0, 0.0, 0.08], sw, sh);
            }
        }

        // Render command overlay
        if let Some(overlay) = overlay {
            let s = self.scale_factor;
            let overlay_w = (500.0 * s).min(sw - 40.0 * s);
            let overlay_h = 60.0 * s;
            let overlay_x = (sw - overlay_w) / 2.0;
            let overlay_y = sh - overlay_h - 40.0 * s;
            let ob = 2.0 * s; // overlay border

            // Background
            Self::push_rect(&mut rect_vertices, &mut rect_indices,
                overlay_x, overlay_y, overlay_w, overlay_h,
                [0.1, 0.1, 0.12, 0.95], sw, sh);
            // Border
            Self::push_rect(&mut rect_vertices, &mut rect_indices,
                overlay_x, overlay_y, overlay_w, ob,
                [0.4, 0.6, 1.0, 0.8], sw, sh);
            Self::push_rect(&mut rect_vertices, &mut rect_indices,
                overlay_x, overlay_y + overlay_h - ob, overlay_w, ob,
                [0.4, 0.6, 1.0, 0.8], sw, sh);
            Self::push_rect(&mut rect_vertices, &mut rect_indices,
                overlay_x, overlay_y, ob, overlay_h,
                [0.4, 0.6, 1.0, 0.8], sw, sh);
            Self::push_rect(&mut rect_vertices, &mut rect_indices,
                overlay_x + overlay_w - ob, overlay_y, ob, overlay_h,
                [0.4, 0.6, 1.0, 0.8], sw, sh);

            // Target label (small, at top)
            let label_y = overlay_y + 6.0 * s;
            let label_color = [0.5, 0.7, 1.0, 1.0];
            for (ci, ch) in overlay.target_label.chars().enumerate() {
                let glyph = self.rasterize_glyph(ch, false, false);
                let cx = overlay_x + 10.0 * s + ci as f32 * self.cell_width;
                if cx + self.cell_width < overlay_x + overlay_w - 10.0 * s {
                    Self::push_glyph(&mut text_vertices, &mut text_indices,
                        cx, label_y, &glyph, label_color, sw, sh);
                }
            }

            // Input text
            let input_y = overlay_y + 28.0 * s;
            let input_color = [0.9, 0.9, 0.95, 1.0];
            let display_text = format!("> {}_", overlay.text);
            for (ci, ch) in display_text.chars().enumerate() {
                let glyph = self.rasterize_glyph(ch, false, false);
                let cx = overlay_x + 10.0 * s + ci as f32 * self.cell_width;
                if cx + self.cell_width < overlay_x + overlay_w - 10.0 * s {
                    Self::push_glyph(&mut text_vertices, &mut text_indices,
                        cx, input_y, &glyph, input_color, sw, sh);
                }
            }
        }

        // Build help overlay into separate buffers so it draws on top of everything
        let mut help_rect_vertices: Vec<RectVertex> = Vec::new();
        let mut help_rect_indices: Vec<u32> = Vec::new();
        let mut help_text_vertices: Vec<Vertex> = Vec::new();
        let mut help_text_indices: Vec<u32> = Vec::new();
        if show_help {
            self.render_help_panel(&mut help_rect_vertices, &mut help_rect_indices,
                &mut help_text_vertices, &mut help_text_indices, sw, sh, help_scroll);
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

            // Draw help overlay on top of everything (separate draw calls so it
            // covers pane text that was already rendered above)
            if !help_rect_indices.is_empty() {
                let help_rect_vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Help Rect VB"),
                    contents: bytemuck::cast_slice(&help_rect_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                let help_rect_ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Help Rect IB"),
                    contents: bytemuck::cast_slice(&help_rect_indices),
                    usage: wgpu::BufferUsages::INDEX,
                });
                pass.set_pipeline(&self.rect_pipeline);
                pass.set_vertex_buffer(0, help_rect_vb.slice(..));
                pass.set_index_buffer(help_rect_ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..help_rect_indices.len() as u32, 0, 0..1);
            }
            if !help_text_indices.is_empty() {
                let help_text_vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Help Text VB"),
                    contents: bytemuck::cast_slice(&help_text_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                let help_text_ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Help Text IB"),
                    contents: bytemuck::cast_slice(&help_text_indices),
                    usage: wgpu::BufferUsages::INDEX,
                });
                pass.set_pipeline(&self.text_pipeline);
                pass.set_bind_group(0, &self.text_bind_group, &[]);
                pass.set_vertex_buffer(0, help_text_vb.slice(..));
                pass.set_index_buffer(help_text_ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..help_text_indices.len() as u32, 0, 0..1);
            }
        }

        // Check for file-based screenshot trigger
        if self.pending_screenshot.is_none() {
            let trigger = "/tmp/termania_screenshot_trigger";
            if let Ok(path) = std::fs::read_to_string(trigger) {
                let path = path.trim().to_string();
                if !path.is_empty() {
                    self.pending_screenshot = Some(path);
                    let _ = std::fs::remove_file(trigger);
                }
            }
        }

        // Screenshot: copy surface texture to buffer if requested
        if let Some(path) = self.pending_screenshot.take() {
            let w = self.width;
            let h = self.height;
            let bytes_per_pixel = 4u32;
            // wgpu requires rows aligned to 256 bytes
            let unpadded_row = w * bytes_per_pixel;
            let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let padded_row = (unpadded_row + align - 1) / align * align;
            let buffer_size = (padded_row * h) as u64;

            let readback_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Screenshot Buffer"),
                size: buffer_size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });

            encoder.copy_texture_to_buffer(
                wgpu::ImageCopyTexture {
                    texture: &output.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::ImageCopyBuffer {
                    buffer: &readback_buffer,
                    layout: wgpu::ImageDataLayout {
                        offset: 0,
                        bytes_per_row: Some(padded_row),
                        rows_per_image: Some(h),
                    },
                },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );

            self.queue.submit(std::iter::once(encoder.finish()));

            // Map the buffer and save as PNG
            let buffer_slice = readback_buffer.slice(..);
            let (tx, rx) = std::sync::mpsc::channel();
            buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
                tx.send(result).unwrap();
            });
            self.device.poll(wgpu::Maintain::Wait);
            if rx.recv().unwrap().is_ok() {
                let data = buffer_slice.get_mapped_range();
                // Remove row padding and convert BGRA->RGBA if needed
                let mut rgba = Vec::with_capacity((w * h * 4) as usize);
                for row in 0..h {
                    let offset = (row * padded_row) as usize;
                    let row_data = &data[offset..offset + (w * 4) as usize];
                    // Surface format is typically BGRA on macOS Metal
                    for pixel in row_data.chunks_exact(4) {
                        rgba.push(pixel[2]); // R (from B position in BGRA)
                        rgba.push(pixel[1]); // G
                        rgba.push(pixel[0]); // B (from R position in BGRA)
                        rgba.push(pixel[3]); // A
                    }
                }
                drop(data);
                readback_buffer.unmap();

                // Save as PNG in a background thread to avoid blocking the render loop
                log::info!("Screenshot: {}x{}, rgba_len={}", w, h, rgba.len());
                let path_clone = path.clone();
                std::thread::spawn(move || {
                    use image::ImageEncoder;
                    match std::fs::File::create(&path_clone) {
                        Ok(file) => {
                            let buf_writer = std::io::BufWriter::new(file);
                            let encoder = image::codecs::png::PngEncoder::new(buf_writer);
                            match encoder.write_image(&rgba, w, h, image::ColorType::Rgba8) {
                                Ok(_) => log::info!("Screenshot saved to {}", path_clone),
                                Err(e) => log::error!("Failed to encode PNG: {}", e),
                            }
                        }
                        Err(e) => log::error!("Failed to create screenshot file: {}", e),
                    }
                });
            }

            output.present();
            return;
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
    }

    /// Request a screenshot on the next render
    pub fn request_screenshot(&mut self, path: String) {
        self.pending_screenshot = Some(path);
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
