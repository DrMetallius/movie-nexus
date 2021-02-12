use std::{
    ptr::null_mut,
    sync::{Mutex, Condvar},
};

use lazy_static::lazy_static;
use windows::ErrorCode;

use bindings::windows::win32::{
    debug::GetLastError,
    dns::{
        DNS_SERVICE_REGISTER_REQUEST, DnsServiceRegister,
    },
    system_services::DNS_REQUEST_PENDING,
    windows_programming::{COMPUTER_NAME_FORMAT, GetComputerNameExW},
};
use wrapper::DnsServiceInfo;

const SERVICE_NAME: &str = "MovieNexus";
const SERVICE_TYPE: &str = "_http._tcp.local";

lazy_static! {
    static ref REGISTRATION_MUTEX: Mutex<()> = Mutex::default();
    static ref REGISTRATION_IN_PROGRESS_MUTEX: Mutex<bool> = Mutex::new(false);
    static ref REGISTRATION_STATE_VAR: Condvar = Condvar::new();
}

#[allow(dead_code)]
mod bindings {
    ::windows::include_bindings!();
}

pub fn register_service(port: u16) -> Result<(), windows::Error> {
    let mut buf = [0u16; 256];
    let mut len = buf.len();

    unsafe { GetComputerNameExW(COMPUTER_NAME_FORMAT::ComputerNameDnsFullyQualified, buf.as_mut_ptr() as _, &mut len as *mut _ as _).ok()?; }

    let first_zero = buf.iter().position(|byte| *byte == 0).unwrap_or(buf.len());
    let hostname = String::from_utf16(&buf[..first_zero]).unwrap();

    let service_name = format!("{}-{}.{}", hostname.clone(), SERVICE_NAME, SERVICE_TYPE);
    let host_name = format!("{}.local", hostname);

    let service_instance = DnsServiceInfo::new(&service_name, &host_name, port);

    let mut request = DNS_SERVICE_REGISTER_REQUEST {
        version: 1,
        interface_index: 0,
        p_service_instance: service_instance.instance(),
        p_register_completion_callback: Some(registration_callback),
        p_query_context: null_mut(),
        h_credentials: Default::default(),
        unicast_enabled: false.into(),
    };

    {
        let _registration_guard = REGISTRATION_MUTEX.lock().unwrap();
        let mut state_guard = REGISTRATION_IN_PROGRESS_MUTEX.lock().unwrap();
        *state_guard = true;

        let result = unsafe { DnsServiceRegister(&mut request as *mut _, null_mut()) };
        if result != DNS_REQUEST_PENDING as u32 {
            return Err(ErrorCode(unsafe { GetLastError() }).into());
        }

        let _var_guard = REGISTRATION_STATE_VAR.wait_while(state_guard, |registration_in_progress| *registration_in_progress).unwrap();
    }

    Ok(())
}

extern "system" fn registration_callback() {
    println!("Service registration complete");

    *REGISTRATION_IN_PROGRESS_MUTEX.lock().unwrap() = false;
    REGISTRATION_STATE_VAR.notify_all();
}

mod wrapper {
    use std::ptr::null_mut;

    use super::bindings::windows::win32::dns::{DNS_SERVICE_INSTANCE, DnsServiceConstructInstance, DnsServiceFreeInstance};

    pub struct DnsServiceInfo {
        instance: *mut DNS_SERVICE_INSTANCE
    }

    impl DnsServiceInfo {
        pub fn new(service_name: &str, host_name: &str, port: u16) -> DnsServiceInfo {
            let instance = unsafe {
                let mut service_name = (service_name.to_owned() + "\0").encode_utf16().collect::<Vec<u16>>();
                let mut host_name = (host_name.to_owned() + "\0").encode_utf16().collect::<Vec<u16>>();

                DnsServiceConstructInstance(
                    service_name.as_mut_ptr(),
                    host_name.as_mut_ptr(),
                    null_mut(),
                    null_mut(),
                    port,
                    0,
                    0,
                    0,
                    null_mut(),
                    null_mut(),
                )
            };

            DnsServiceInfo {
                instance
            }
        }

        pub fn instance(&self) -> *mut DNS_SERVICE_INSTANCE {
            self.instance
        }
    }

    impl Drop for DnsServiceInfo {
        fn drop(&mut self) {
            unsafe { DnsServiceFreeInstance(self.instance) }
        }
    }
}