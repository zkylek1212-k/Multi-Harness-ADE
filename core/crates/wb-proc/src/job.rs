//! Windows Job Object: agents die with the app, including when the app crashes.
//!
//! Plain kill-on-close would also take down independent programs started from a terminal (a browser, an editor),
//! which `taskkill /T` does not do today. So a clean exit calls [`KillOnCloseJob::release`], which clears the flag:
//! normal quit keeps today's behaviour, and only a crash falls back to killing everything in the job.

#[cfg(windows)]
mod imp {
    use std::io;
    use windows_sys::Win32::Foundation::{CloseHandle, FALSE, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

    pub struct KillOnCloseJob {
        handle: HANDLE,
    }

    unsafe impl Send for KillOnCloseJob {}
    unsafe impl Sync for KillOnCloseJob {}

    impl KillOnCloseJob {
        pub fn new() -> io::Result<Self> {
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self { handle };
            job.set_limit(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE)?;
            Ok(job)
        }

        fn set_limit(&self, flags: u32) -> io::Result<()> {
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            info.BasicLimitInformation.LimitFlags = flags;
            let ok = unsafe {
                SetInformationJobObject(self.handle, JobObjectExtendedLimitInformation, &info as *const _ as *const _, std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32)
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        pub fn assign(&self, pid: u32) -> io::Result<()> {
            let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, FALSE, pid) };
            if process.is_null() {
                return Err(io::Error::last_os_error());
            }
            let ok = unsafe { AssignProcessToJobObject(self.handle, process) };
            let err = io::Error::last_os_error();
            unsafe { CloseHandle(process) };
            if ok == 0 {
                return Err(err);
            }
            Ok(())
        }

        /// Clean exit: stop killing on close, so independent programs started from a terminal survive.
        pub fn release(&self) -> io::Result<()> {
            self.set_limit(0)
        }
    }

    impl Drop for KillOnCloseJob {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.handle) };
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::io;
    /// Windows only: other platforms use process groups (see `runner::kill_tree`).
    pub struct KillOnCloseJob;
    impl KillOnCloseJob {
        pub fn new() -> io::Result<Self> {
            Ok(Self)
        }
        pub fn assign(&self, _pid: u32) -> io::Result<()> {
            Ok(())
        }
        pub fn release(&self) -> io::Result<()> {
            Ok(())
        }
    }
}

pub use imp::KillOnCloseJob;

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    fn alive(pid: u32) -> bool {
        let out = Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
    }

    fn wait_gone(pid: u32) -> bool {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(5) {
            if !alive(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    /// node spawns a detached grandchild and prints its pid. Detached = what `start chrome` looks like to us.
    fn spawn_with_detached_child() -> (std::process::Child, u32) {
        let script = "const c=require('child_process').spawn(process.execPath,['-e','setInterval(()=>{},1000)'],{detached:true,stdio:'ignore'});console.log(c.pid);setInterval(()=>{},1000)";
        let mut parent = Command::new("node").args(["-e", script]).stdout(Stdio::piped()).spawn().unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(parent.stdout.take().unwrap()), &mut line).unwrap();
        (parent, line.trim().parse().unwrap())
    }

    #[test]
    fn crash_kills_the_whole_tree_even_detached_children() {
        let job = KillOnCloseJob::new().unwrap();
        // Assign before the grandchild exists: spawn a parent that waits, then let it spawn.
        let script = "setTimeout(()=>{const c=require('child_process').spawn(process.execPath,['-e','setInterval(()=>{},1000)'],{detached:true,stdio:'ignore'});console.log(c.pid)},500);setInterval(()=>{},1000)";
        let mut parent = Command::new("node").args(["-e", script]).stdout(Stdio::piped()).spawn().unwrap();
        job.assign(parent.id()).unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(parent.stdout.take().unwrap()), &mut line).unwrap();
        let child: u32 = line.trim().parse().unwrap();
        assert!(alive(child));
        drop(job); // what happens when the app process dies: the last handle closes
        assert!(wait_gone(parent.id()), "parent survived");
        assert!(wait_gone(child), "detached child survived a crash");
        let _ = parent.wait();
    }

    #[test]
    fn clean_exit_keeps_independent_programs() {
        let job = KillOnCloseJob::new().unwrap();
        let script = "setTimeout(()=>{const c=require('child_process').spawn(process.execPath,['-e','setTimeout(()=>{},8000)'],{detached:true,stdio:'ignore'});console.log(c.pid)},500);setInterval(()=>{},1000)";
        let mut parent = Command::new("node").args(["-e", script]).stdout(Stdio::piped()).spawn().unwrap();
        job.assign(parent.id()).unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(parent.stdout.take().unwrap()), &mut line).unwrap();
        let child: u32 = line.trim().parse().unwrap();
        job.release().unwrap();
        drop(job);
        std::thread::sleep(Duration::from_millis(500));
        assert!(alive(child), "independent program was killed on a clean exit");
        let _ = Command::new("taskkill").args(["/PID", &child.to_string(), "/T", "/F"]).output();
        let _ = parent.kill();
        let _ = parent.wait();
        let _ = spawn_with_detached_child; // helper kept for readers comparing both shapes
    }
}
