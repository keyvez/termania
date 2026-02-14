use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

/// Manages a pseudo-terminal (PTY) for a child process
pub struct Pty {
    master: OwnedFd,
    child_pid: u32,
}

impl Pty {
    /// Spawn a new PTY with the given dimensions, running a shell
    pub fn spawn(
        cols: u16,
        rows: u16,
        shell: Option<&str>,
        cwd: Option<&str>,
    ) -> io::Result<Self> {
        // Open a PTY master/slave pair
        let result = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        let master_fd = unsafe { OwnedFd::from_raw_fd(result) };

        // Grant access and unlock
        if unsafe { libc::grantpt(master_fd.as_raw_fd()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::unlockpt(master_fd.as_raw_fd()) } != 0 {
            return Err(io::Error::last_os_error());
        }

        // Get slave name
        let slave_name = unsafe {
            let ptr = libc::ptsname(master_fd.as_raw_fd());
            if ptr.is_null() {
                return Err(io::Error::last_os_error());
            }
            std::ffi::CStr::from_ptr(ptr).to_owned()
        };

        // Set window size on master
        let winsize = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        unsafe {
            libc::ioctl(master_fd.as_raw_fd(), libc::TIOCSWINSZ, &winsize);
        }

        // Fork
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(io::Error::last_os_error());
        }

        if pid == 0 {
            // Child process
            unsafe {
                // Create new session
                libc::setsid();

                // Open slave PTY
                let slave_fd = libc::open(slave_name.as_ptr(), libc::O_RDWR);
                if slave_fd < 0 {
                    libc::_exit(1);
                }

                // Set as controlling terminal
                libc::ioctl(slave_fd, libc::TIOCSCTTY as libc::c_ulong, 0);

                // Set window size on slave too
                libc::ioctl(slave_fd, libc::TIOCSWINSZ, &winsize);

                // Redirect stdin/stdout/stderr to slave PTY
                libc::dup2(slave_fd, 0);
                libc::dup2(slave_fd, 1);
                libc::dup2(slave_fd, 2);

                if slave_fd > 2 {
                    libc::close(slave_fd);
                }

                // Close the master fd in child
                // (OwnedFd will be dropped, but we explicitly close to be safe)
                libc::close(master_fd.as_raw_fd());

                // Change directory if requested
                if let Some(dir) = cwd {
                    if let Ok(dir_c) = CString::new(dir) {
                        libc::chdir(dir_c.as_ptr());
                    }
                }

                // Set TERM environment variable
                let term_env = CString::new("TERM=xterm-256color").unwrap();
                libc::putenv(term_env.as_ptr() as *mut _);

                // Set COLORTERM
                let colorterm = CString::new("COLORTERM=truecolor").unwrap();
                libc::putenv(colorterm.as_ptr() as *mut _);

                // Exec the shell
                let shell_path = shell
                    .map(|s| s.to_string())
                    .or_else(|| std::env::var("SHELL").ok())
                    .unwrap_or_else(|| "/bin/bash".to_string());

                let shell_c = CString::new(shell_path.as_str()).unwrap();
                let shell_name = CString::new(
                    std::path::Path::new(&shell_path)
                        .file_name()
                        .unwrap_or_default()
                        .to_str()
                        .unwrap_or("sh"),
                )
                .unwrap();

                // Login shell (prefix with -)
                let login_name = CString::new(format!(
                    "-{}",
                    shell_name.to_str().unwrap_or("sh")
                ))
                .unwrap();

                let args = [login_name.as_ptr(), std::ptr::null()];
                libc::execvp(shell_c.as_ptr(), args.as_ptr());

                // If exec fails
                libc::_exit(1);
            }
        }

        // Parent process
        // Set master to non-blocking
        unsafe {
            let flags = libc::fcntl(master_fd.as_raw_fd(), libc::F_GETFL);
            libc::fcntl(
                master_fd.as_raw_fd(),
                libc::F_SETFL,
                flags | libc::O_NONBLOCK,
            );
        }

        // Forget the OwnedFd for the master since we'll manage it ourselves
        // Actually, we keep the OwnedFd and it will close on drop
        Ok(Self {
            master: master_fd,
            child_pid: pid as u32,
        })
    }

    /// Write data to the PTY master (sends to child's stdin)
    pub fn write(&self, data: &[u8]) -> io::Result<usize> {
        let result = unsafe {
            libc::write(
                self.master.as_raw_fd(),
                data.as_ptr() as *const _,
                data.len(),
            )
        };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(result as usize)
        }
    }

    /// Read data from the PTY master (receives child's stdout)
    pub fn read(&self) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; 65536];
        let result = unsafe {
            libc::read(
                self.master.as_raw_fd(),
                buf.as_mut_ptr() as *mut _,
                buf.len(),
            )
        };
        if result > 0 {
            buf.truncate(result as usize);
            Some(buf)
        } else {
            None
        }
    }

    /// Check if the child process is still alive
    pub fn is_alive(&self) -> bool {
        let mut status: libc::c_int = 0;
        let result = unsafe { libc::waitpid(self.child_pid as i32, &mut status, libc::WNOHANG) };
        // waitpid returns 0 if child is still running, >0 if exited, -1 on error
        result == 0
    }

    /// Get the child process PID
    pub fn child_pid(&self) -> u32 {
        self.child_pid
    }

    /// Resize the PTY
    pub fn resize(&self, cols: u16, rows: u16) {
        let winsize = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        unsafe {
            libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &winsize);
        }
    }
}
