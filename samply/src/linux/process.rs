use libc::execvp;
use nix::sys::wait::WaitStatus;
use nix::unistd::Pid;

use std::ffi::{CString, OsStr, OsString};
use std::os::fd::{AsFd, AsRawFd, IntoRawFd, OwnedFd};
use std::os::raw::c_char;
use std::os::unix::prelude::{ExitStatusExt, OsStrExt};
use std::process::ExitStatus;

/// Allows launching a command in a suspended state, so that we can know its
/// pid and initialize profiling before proceeding to execute the command.
pub struct SuspendedLaunchedProcess {
    pid: Pid,
    send_end_of_resume_pipe: OwnedFd,
    recv_end_of_execerr_pipe: OwnedFd,
}

impl SuspendedLaunchedProcess {
    pub fn launch_in_suspended_state(
        command_name: &OsStr,
        command_args: &[OsString],
    ) -> std::io::Result<Self> {
        let argv: Vec<CString> = std::iter::once(command_name)
            .chain(command_args.iter().map(|s| s.as_os_str()))
            .map(|os_str: &OsStr| CString::new(os_str.as_bytes().to_vec()).unwrap())
            .collect();
        let argv: Vec<*const c_char> = argv
            .iter()
            .map(|c_str| c_str.as_ptr())
            .chain(std::iter::once(std::ptr::null()))
            .collect();

        let (resume_rp, resume_sp) = nix::unistd::pipe2(nix::fcntl::OFlag::O_CLOEXEC).unwrap();
        let (execerr_rp, execerr_sp) = nix::unistd::pipe2(nix::fcntl::OFlag::O_CLOEXEC).unwrap();

        match unsafe { nix::unistd::fork() }.expect("Fork failed") {
            nix::unistd::ForkResult::Child => {
                // std::panic::always_abort();
                nix::unistd::close(resume_sp.into_raw_fd()).unwrap();
                nix::unistd::close(execerr_rp.into_raw_fd()).unwrap();
                Self::run_child(resume_rp, execerr_sp, &argv)
            }
            nix::unistd::ForkResult::Parent { child } => {
                nix::unistd::close(resume_rp.into_raw_fd())?;
                nix::unistd::close(execerr_sp.into_raw_fd())?;
                Ok(Self {
                    pid: child,
                    send_end_of_resume_pipe: resume_sp,
                    recv_end_of_execerr_pipe: execerr_rp,
                })
            }
        }
    }

    pub fn pid(&self) -> u32 {
        self.pid.as_raw() as u32
    }

    const EXECERR_MSG_FOOTER: [u8; 4] = *b"NOEX";

    pub fn unsuspend_and_run(self) -> std::io::Result<RunningProcess> {
        // Send a byte to the child process.
        nix::unistd::write(self.send_end_of_resume_pipe.as_fd(), &[0x42])?;
        nix::unistd::close(self.send_end_of_resume_pipe.into_raw_fd())?;

        // Wait for the child to indicate success or failure of the execve call.
        // loop for EINTR
        loop {
            let mut bytes = [0; 8];
            let read_result =
                nix::unistd::read(self.recv_end_of_execerr_pipe.as_raw_fd(), &mut bytes);

            // The parent has replied! Or exited.
            match read_result {
                Ok(0) => {
                    // The child closed the pipe.
                    // This means that execution was successful.
                    break;
                }
                Ok(8) => {
                    // We got an execerr message from the child. This means that the execve call failed.
                    // Decode the message.
                    let (errno, footer) = bytes.split_at(4);
                    assert_eq!(
                        Self::EXECERR_MSG_FOOTER,
                        footer,
                        "Validation on the execerr pipe failed: {bytes:?}",
                    );
                    let errno = i32::from_be_bytes([errno[0], errno[1], errno[2], errno[3]]);
                    let _wait_status = nix::sys::wait::waitpid(self.pid, None);
                    return Err(std::io::Error::from_raw_os_error(errno));
                }
                Ok(_) => {
                    // We got a message that was shorter or longer than the expected 8 bytes.
                    // It should never be shorter than 8 bytes because pipe I/O up to PIPE_BUF bytes
                    // should be atomic.

                    // This case is very unexpected and we will panic, after making sure that the child has
                    // fully executed.
                    let _status = nix::sys::wait::waitpid(self.pid, None)
                        .expect("waitpid should always succeed");

                    panic!("short read on the execerr pipe")
                }
                Err(nix::errno::Errno::EINTR) => {}
                Err(_) => std::process::exit(1),
            }
        }

        Ok(RunningProcess { pid: self.pid })
    }

    /// Executed in the forked child process. This function never returns.
    fn run_child(
        recv_end_of_resume_pipe: OwnedFd,
        send_end_of_execerr_pipe: OwnedFd,
        argv: &[*const c_char],
    ) -> ! {
        // Wait for the parent to send us a byte through the pipe.
        // This will signal us to start executing.

        // loop to handle EINTR
        loop {
            let mut buf = [0];
            let read_result = nix::unistd::read(recv_end_of_resume_pipe.as_raw_fd(), &mut buf);

            // The parent has replied! Or exited.
            match read_result {
                Ok(0) => {
                    // The parent closed the pipe without telling us to start.
                    // This usually means that it encountered a problem when it tried to start
                    // profiling; in that case it just terminates, causing the pipe to close.
                    // End this process and do not execute the to-be-launched command.
                    std::process::exit(0)
                }
                Ok(_) => {
                    // The parent signaled that we can start. Exec!
                    let _ = unsafe { execvp(argv[0], argv.as_ptr()) };

                    // If executing went well, we don't get here. In that case, `send_end_of_execerr_pipe`
                    // is now closed, and the parent will notice this and proceed.

                    // But we got here! This can happen if the command doesn't exist.
                    // Return the error number via the "execerr" pipe.
                    let errno = nix::errno::Errno::last_raw().to_be_bytes();
                    let bytes = [
                        errno[0],
                        errno[1],
                        errno[2],
                        errno[3],
                        Self::EXECERR_MSG_FOOTER[0],
                        Self::EXECERR_MSG_FOOTER[1],
                        Self::EXECERR_MSG_FOOTER[2],
                        Self::EXECERR_MSG_FOOTER[3],
                    ];
                    // Send `bytes` through the pipe.
                    // Pipe I/O up to PIPE_BUF bytes should be atomic.
                    nix::unistd::write(send_end_of_execerr_pipe, &bytes).unwrap();
                    // Terminate the child process and *don't* run `at_exit` destructors as
                    // we're being torn down regardless.
                    unsafe { libc::_exit(1) }
                }
                Err(nix::errno::Errno::EINTR) => {}
                Err(_) => std::process::exit(1),
            }
        }
    }
}

pub struct RunningProcess {
    pid: Pid,
}

impl RunningProcess {
    pub fn wait(self) -> Result<std::process::ExitStatus, nix::errno::Errno> {
        let wait_status = nix::sys::wait::waitpid(self.pid, None)?;
        let exit_status = match wait_status {
            WaitStatus::Exited(_pid, exit_code) => ExitStatus::from_raw(exit_code),
            wait_status => {
                panic!("Unexpected waitpid result: {wait_status:?}");
            }
        };
        Ok(exit_status)
    }
}
