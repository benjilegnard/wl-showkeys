/*
 * Portions of this file taken from wlroots; MIT licensed. Its purpose is to
 * run a child process as root for opening evdev devices.
 *
 * NOTICE: Most of this code runs as root.
 */

use nix::sys::socket::{
    recvmsg, sendmsg, socketpair, AddressFamily, ControlMessage, ControlMessageOwned, MsgFlags,
    SockFlag, SockType,
};
use nix::sys::wait::waitpid;
use nix::unistd::{fork, geteuid, getgid, getuid, setgid, setuid, ForkResult, Pid};
use std::io;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};

const PATH_MAX: usize = 4096;

#[repr(u8)]
#[derive(Debug, Clone, Copy)]
enum MsgType {
    Open = 0,
    End = 1,
}

impl MsgType {
    fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(MsgType::Open),
            1 => Some(MsgType::End),
            _ => None,
        }
    }
}

#[repr(C)]
struct Msg {
    msg_type: u8,
    path: [u8; PATH_MAX],
}

impl Msg {
    fn new(msg_type: MsgType) -> Self {
        Self {
            msg_type: msg_type as u8,
            path: [0; PATH_MAX],
        }
    }

    fn set_path(&mut self, path: &str) {
        let bytes = path.as_bytes();
        let len = bytes.len().min(PATH_MAX - 1);
        self.path[..len].copy_from_slice(&bytes[..len]);
        self.path[len] = 0;
    }

    fn get_path(&self) -> &str {
        let end = self
            .path
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(PATH_MAX);
        std::str::from_utf8(&self.path[..end]).unwrap_or("")
    }
}

fn recv_msg(sock: RawFd) -> Result<(Msg, Option<RawFd>), Box<dyn std::error::Error>> {
    let mut msg: Msg = Msg::new(MsgType::Open);
    let msg_bytes = unsafe {
        std::slice::from_raw_parts_mut(
            &mut msg as *mut Msg as *mut u8,
            std::mem::size_of::<Msg>(),
        )
    };

    let mut iov = [io::IoSliceMut::new(msg_bytes)];
    let mut cmsg_space = nix::cmsg_space!([RawFd; 1]);

    let msg_result = recvmsg::<()>(sock, &mut iov, Some(&mut cmsg_space), MsgFlags::MSG_CMSG_CLOEXEC)?;

    let mut fd = None;
    if let Ok(cmsg_iter) = msg_result.cmsgs() {
        for cmsg in cmsg_iter {
            if let ControlMessageOwned::ScmRights(fds) = cmsg {
                if !fds.is_empty() {
                    fd = Some(fds[0]);
                }
            }
        }
    }

    Ok((msg, fd))
}

fn send_msg(sock: RawFd, fd: Option<RawFd>, msg: &Msg) -> Result<(), Box<dyn std::error::Error>> {
    let msg_bytes = unsafe {
        std::slice::from_raw_parts(msg as *const Msg as *const u8, std::mem::size_of::<Msg>())
    };

    let iov = [io::IoSlice::new(msg_bytes)];

    if let Some(fd) = fd {
        let fds = [fd];
        let cmsg = vec![ControlMessage::ScmRights(&fds)];
        sendmsg::<()>(sock, &iov, &cmsg, MsgFlags::empty(), None)?;
    } else {
        let cmsg: Vec<ControlMessage> = vec![];
        sendmsg::<()>(sock, &iov, &cmsg, MsgFlags::empty(), None)?;
    }
    Ok(())
}

fn devmgr_run(sockfd: RawFd, devpath: &str) -> ! {
    loop {
        let (msg, _) = match recv_msg(sockfd) {
            Ok(res) => res,
            Err(_) => std::process::exit(0),
        };

        let msg_type = match MsgType::from_u8(msg.msg_type) {
            Some(t) => t,
            None => std::process::exit(1),
        };

        match msg_type {
            MsgType::Open => {
                let path = msg.get_path();

                // Security check: ensure path starts with devpath
                if !path.starts_with(devpath) {
                    eprintln!("devmgr: Attempted path traversal: {}", path);
                    std::process::exit(1);
                }

                let fd = unsafe {
                    libc::open(
                        path.as_ptr() as *const libc::c_char,
                        libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOCTTY | libc::O_NONBLOCK,
                    )
                };

                let errno = if fd >= 0 { 0 } else { io::Error::last_os_error().raw_os_error().unwrap_or(1) };

                let mut response = Msg::new(MsgType::Open);
                let errno_bytes = errno.to_ne_bytes();
                response.path[..4].copy_from_slice(&errno_bytes);

                let send_fd = if fd >= 0 { Some(fd) } else { None };
                let _ = send_msg(sockfd, send_fd, &response);

                if fd >= 0 {
                    unsafe {
                        libc::close(fd);
                    }
                }
            }
            MsgType::End => {
                let response = Msg::new(MsgType::End);
                let _ = send_msg(sockfd, None, &response);
                std::process::exit(0);
            }
        }
    }
}

pub struct DevMgr {
    pub fd: RawFd,
    pub pid: Pid,
}

impl DevMgr {
    pub fn start(devpath: &str) -> Result<Self, Box<dyn std::error::Error>> {
        if geteuid().as_raw() != 0 {
            return Err("wshowkeys needs to be setuid to read input events".into());
        }

        let (sock0, sock1) = socketpair(
            AddressFamily::Unix,
            SockType::SeqPacket,
            None,
            SockFlag::empty(),
        )?;

        match unsafe { fork()? } {
            ForkResult::Parent { child } => {
                nix::unistd::close(sock1.as_raw_fd())?;

                // Drop root privileges
                setgid(getgid())?;
                setuid(getuid())?;

                // Verify we can't regain root
                if setuid(nix::unistd::Uid::from_raw(0)).is_ok() {
                    return Err("devmgr: failed to drop root".into());
                }

                Ok(DevMgr {
                    fd: sock0.as_raw_fd(),
                    pid: child,
                })
            }
            ForkResult::Child => {
                nix::unistd::close(sock0.as_raw_fd()).unwrap();
                let devpath_owned = devpath.to_string();
                devmgr_run(sock1.as_raw_fd(), &devpath_owned);
            }
        }
    }

    pub fn open(&self, path: &str) -> Result<RawFd, Box<dyn std::error::Error>> {
        let mut msg = Msg::new(MsgType::Open);
        msg.set_path(path);

        send_msg(self.fd, None, &msg)?;

        for retry in 0..3 {
            match recv_msg(self.fd) {
                Ok((response, fd)) => {
                    let errno_bytes: [u8; 4] = response.path[..4].try_into().unwrap();
                    let errno = i32::from_ne_bytes(errno_bytes);

                    if errno != 0 {
                        return Err(format!("Failed to open device: errno {}", errno).into());
                    }

                    return fd.ok_or_else(|| "No file descriptor received".into());
                }
                Err(e) if retry < 2 => continue,
                Err(e) => return Err(e),
            }
        }

        Err("Failed after retries".into())
    }

    pub fn finish(self) {
        let msg = Msg::new(MsgType::End);
        let _ = send_msg(self.fd, None, &msg);
        let _ = recv_msg(self.fd);
        let _ = waitpid(self.pid, None);
        let _ = nix::unistd::close(self.fd);
    }
}
