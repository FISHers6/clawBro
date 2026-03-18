pub mod acp_agent;
pub mod native_runtime;
pub mod team;

pub fn install_rustls_default() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
