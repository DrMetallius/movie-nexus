fn main() {
    windows::build!(
        windows::win32::debug::GetLastError,
        windows::win32::dns::{DNS_SERVICE_REGISTER_REQUEST, DnsServiceConstructInstance, DnsServiceRegister, DnsServiceFreeInstance},
        windows::win32::system_services::DNS_REQUEST_PENDING,
        windows::win32::windows_programming::{COMPUTER_NAME_FORMAT, GetComputerNameExW},
    );
}