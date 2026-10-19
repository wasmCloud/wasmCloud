//! Stable Win32 APIs for race-free assignment to a kill-on-close job.

use anyhow::{Context as _, Result, bail};
use std::{
    io,
    os::windows::{
        io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle, RawHandle},
        process::CommandExt as _,
    },
    process::Command,
};
use windows_sys::Win32::{
    Foundation::INVALID_HANDLE_VALUE,
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicProcessIdList,
            JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
        },
        Threading::{CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
    },
};

pub(super) struct Job(OwnedHandle);

impl Job {
    pub(super) fn members(&self) -> Option<Vec<u32>> {
        // This is JOBOBJECT_BASIC_PROCESS_ID_LIST with room for its variable
        // trailing array. A larger job is explicitly unmeasurable, not partial.
        #[repr(C)]
        struct ProcessIds {
            assigned: u32,
            count: u32,
            ids: [usize; super::MAX_PROCESS_MEMBERS],
        }
        let mut processes = ProcessIds {
            assigned: 0,
            count: 0,
            ids: [0; super::MAX_PROCESS_MEMBERS],
        };
        // SAFETY: the C layout matches the selected information class, the
        // writable buffer size is exact, and the job handle remains owned.
        #[allow(unsafe_code)]
        let queried = unsafe {
            QueryInformationJobObject(
                self.0.as_raw_handle(),
                JobObjectBasicProcessIdList,
                std::ptr::from_mut(&mut processes).cast(),
                std::mem::size_of_val(&processes) as u32,
                std::ptr::null_mut(),
            )
        };
        if queried == 0 || processes.count != processes.assigned {
            return None;
        }
        processes
            .ids
            .get(..processes.count as usize)?
            .iter()
            .map(|pid| u32::try_from(*pid).ok())
            .collect()
    }

    pub(super) fn prepare(command: &mut Command) -> Result<Self> {
        // SAFETY: null attributes/name create an unnamed, non-inheritable job.
        #[allow(unsafe_code)]
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error()).context("creating process job");
        }
        // SAFETY: CreateJobObjectW returned a new, valid handle owned by us.
        #[allow(unsafe_code)]
        let job = Self(unsafe { OwnedHandle::from_raw_handle(handle) });
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: limits has the layout and size selected by the information class.
        #[allow(unsafe_code)]
        let set = unsafe {
            SetInformationJobObject(
                job.0.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        };
        if set == 0 {
            return Err(io::Error::last_os_error()).context("setting process job limits");
        }
        command.creation_flags(CREATE_SUSPENDED);
        Ok(job)
    }

    pub(super) fn assign_and_resume(&self, pid: u32, process: RawHandle) -> Result<()> {
        // SAFETY: the caller retains the newly created, suspended child handle.
        // It kills and reaps that child if any part of this operation fails.
        #[allow(unsafe_code)]
        let assigned = unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), process) };
        if assigned == 0 {
            return Err(io::Error::last_os_error()).context("assigning child to process job");
        }
        // std::process::Child does not expose the initial thread handle. Since
        // CREATE_SUSPENDED prevented user code from running, its initial thread
        // is still present and can be found through the documented thread list.
        // SAFETY: this snapshot requests only thread metadata and no raw pointers.
        #[allow(unsafe_code)]
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error()).context("listing suspended child threads");
        }
        // SAFETY: the snapshot is a valid, newly allocated handle.
        #[allow(unsafe_code)]
        let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        // SAFETY: entry is initialized and its declared size matches its allocation.
        #[allow(unsafe_code)]
        let mut found = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) };
        while found != 0 {
            if entry.th32OwnerProcessID == pid {
                // SAFETY: the child is still suspended and retained by the caller,
                // so its initial thread cannot exit or have its ID reused here.
                #[allow(unsafe_code)]
                let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                if thread.is_null() {
                    return Err(io::Error::last_os_error())
                        .context("opening suspended child thread");
                }
                // SAFETY: OpenThread returned a new valid handle owned by us.
                #[allow(unsafe_code)]
                let thread = unsafe { OwnedHandle::from_raw_handle(thread) };
                // SAFETY: the thread belongs to our suspended child, now in the job.
                #[allow(unsafe_code)]
                let suspended = unsafe { ResumeThread(thread.as_raw_handle()) };
                if suspended == u32::MAX {
                    return Err(io::Error::last_os_error()).context("resuming child process");
                }
                if suspended == 1 {
                    return Ok(());
                }
                if suspended > 1 {
                    bail!("the child thread is suspended by another controller");
                }
                // An injected thread may already be running; keep looking for
                // the initial thread suspended by CREATE_SUSPENDED.
            }
            entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            // SAFETY: the snapshot and writable entry remain valid for this call.
            #[allow(unsafe_code)]
            {
                found = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) };
            }
        }
        bail!("the suspended child has no resumable thread")
    }
}
