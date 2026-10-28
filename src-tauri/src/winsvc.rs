//! Read-only Windows service queries. Starting a service needs admin and lives
//! in `procs` (through a UAC prompt).

use windows::core::HSTRING;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD};
use windows::Win32::System::Services::{
    CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_MANAGER_CONNECT,
    SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_STATUS,
};

/// `Some(running)` if the service exists, `None` if it does not.
pub fn query(name: &str) -> Option<bool> {
    unsafe {
        let scm = OpenSCManagerW(None, None, SC_MANAGER_CONNECT).ok()?;
        let result = match OpenServiceW(scm, &HSTRING::from(name), SERVICE_QUERY_STATUS) {
            Ok(svc) => {
                let mut st = SERVICE_STATUS::default();
                let running = QueryServiceStatus(svc, &mut st).is_ok() && st.dwCurrentState == SERVICE_RUNNING;
                let _ = CloseServiceHandle(svc);
                Some(running)
            }
            Err(_) => None,
        };
        let _ = CloseServiceHandle(scm);
        result
    }
}

/// The service's start type from the registry: 2 Automatic, 3 Manual, 4 Disabled.
pub fn start_type(name: &str) -> Option<u32> {
    let key = HSTRING::from(format!("SYSTEM\\CurrentControlSet\\Services\\{name}"));
    let value = HSTRING::from("Start");
    let mut data = 0u32;
    let mut size = 4u32;
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            &key,
            &value,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut size),
        )
        .ok()
        .ok()?;
    }
    Some(data)
}
