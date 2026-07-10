// Portions of this file taken from sway, MIT licensed
use cairo::{Context, Format, ImageSurface};
use nix::sys::mman::{mmap, munmap, MapFlags, ProtFlags};
use nix::time::{clock_gettime, ClockId};
use std::ffi::CString;
use std::os::fd::{AsFd, OwnedFd, FromRawFd};
use std::os::unix::io::RawFd;
use std::ptr::NonNull;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_shm::WlShm;

const SHM_RETRIES: i32 = 100;

pub struct PoolBuffer {
    pub buffer: Option<WlBuffer>,
    pub surface: Option<ImageSurface>,
    pub cairo: Option<Context>,
    pub width: u32,
    pub height: u32,
    data: Option<NonNull<libc::c_void>>,
    size: usize,
    pub busy: bool,
}

impl PoolBuffer {
    pub fn new() -> Self {
        Self {
            buffer: None,
            surface: None,
            cairo: None,
            width: 0,
            height: 0,
            data: None,
            size: 0,
            busy: false,
        }
    }
}

impl Drop for PoolBuffer {
    fn drop(&mut self) {
        self.destroy();
    }
}

impl PoolBuffer {
    pub fn destroy(&mut self) {
        if let Some(buffer) = self.buffer.take() {
            buffer.destroy();
        }

        self.cairo = None;
        self.surface = None;

        if let Some(data) = self.data.take() {
            unsafe {
                let _ = munmap(data, self.size);
            }
        }

        self.width = 0;
        self.height = 0;
        self.size = 0;
        self.busy = false;
    }
}

fn randname(buf: &mut [u8; 6]) {
    let ts = clock_gettime(ClockId::CLOCK_REALTIME).unwrap();
    let mut r = ts.tv_nsec();

    for i in 0..6 {
        buf[i] = b'A' + ((r & 15) as u8) + (((r & 16) * 2) as u8);
        r >>= 5;
    }
}

fn create_shm_file() -> Result<RawFd, std::io::Error> {
    for _ in 0..SHM_RETRIES {
        let mut name_bytes = [0u8; 6];
        randname(&mut name_bytes);

        let name = format!(
            "/wl_shm-{}",
            std::str::from_utf8(&name_bytes).unwrap()
        );

        let c_name = CString::new(name.as_bytes()).unwrap();

        let fd = unsafe {
            libc::shm_open(
                c_name.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )
        };

        if fd >= 0 {
            unsafe {
                libc::shm_unlink(c_name.as_ptr());
            }
            return Ok(fd);
        }

        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EEXIST) {
            return Err(err);
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "Failed to create shm file after retries",
    ))
}

pub fn allocate_shm_file(size: usize) -> Result<RawFd, std::io::Error> {
    let fd = create_shm_file()?;

    let ret = unsafe { libc::ftruncate(fd, size as i64) };
    if ret < 0 {
        let err = std::io::Error::last_os_error();
        unsafe {
            libc::close(fd);
        }
        return Err(err);
    }

    Ok(fd)
}

fn create_buffer<T>(
    shm: &WlShm,
    buf: &mut PoolBuffer,
    width: u32,
    height: u32,
    qh: &wayland_client::QueueHandle<T>,
) -> Result<(), Box<dyn std::error::Error>>
where
    T: wayland_client::Dispatch<wayland_client::protocol::wl_shm_pool::WlShmPool, ()> + wayland_client::Dispatch<wayland_client::protocol::wl_buffer::WlBuffer, ()> + 'static,
{
    let stride = width * 4;
    let size = (stride * height) as usize;

    let fd = allocate_shm_file(size)?;
    let owned_fd = unsafe { OwnedFd::from_raw_fd(fd) };

    let data = unsafe {
        mmap(
            None,
            std::num::NonZeroUsize::new(size).unwrap(),
            ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
            MapFlags::MAP_SHARED,
            &owned_fd,
            0,
        )?
    };

    let pool = shm.create_pool(owned_fd.as_fd(), size as i32, qh, ());

    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        stride as i32,
        wayland_client::protocol::wl_shm::Format::Argb8888,
        qh,
        (),
    );

    pool.destroy();

    buf.size = size;
    buf.width = width;
    buf.height = height;
    buf.data = Some(data);

    // Create Cairo surface from the mmap'd data
    let surface = unsafe {
        ImageSurface::create_for_data_unsafe(
            data.as_ptr() as *mut u8,
            Format::ARgb32,
            width as i32,
            height as i32,
            stride as i32,
        )?
    };

    let cairo = Context::new(&surface)?;

    buf.surface = Some(surface);
    buf.cairo = Some(cairo);
    buf.buffer = Some(buffer);

    Ok(())
}

pub fn get_next_buffer<'a, T>(
    shm: &WlShm,
    pool: &'a mut [PoolBuffer; 2],
    width: u32,
    height: u32,
    qh: &wayland_client::QueueHandle<T>,
) -> Result<&'a mut PoolBuffer, Box<dyn std::error::Error>>
where
    T: wayland_client::Dispatch<wayland_client::protocol::wl_shm_pool::WlShmPool, ()> + wayland_client::Dispatch<wayland_client::protocol::wl_buffer::WlBuffer, ()> + 'static,
{
    let mut buffer_idx = None;

    for (i, buf) in pool.iter().enumerate() {
        if !buf.busy {
            buffer_idx = Some(i);
            break;
        }
    }

    let buffer_idx = buffer_idx.ok_or("No free buffers available")?;
    let buffer = &mut pool[buffer_idx];

    if buffer.width != width || buffer.height != height {
        buffer.destroy();
    }

    if buffer.buffer.is_none() {
        create_buffer(shm, buffer, width, height, qh)?;
    }

    buffer.busy = true;
    Ok(buffer)
}
