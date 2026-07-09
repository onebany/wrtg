//! Shared TLS client configuration with public-root certificate validation.

use std::sync::{Arc, LazyLock};

use rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;

static CLIENT_CONFIG: LazyLock<Arc<ClientConfig>> = LazyLock::new(|| {
    let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    Arc::new(
        ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
});

pub fn connector() -> TlsConnector {
    TlsConnector::from(CLIENT_CONFIG.clone())
}
