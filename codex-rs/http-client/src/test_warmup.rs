//! Test-only macOS Secure Transport warmup shared by native-TLS tests.
//!
//! The first Secure Transport context creation in a test process can take
//! several seconds (Security framework one-time initialization inside
//! `SSLCreateContext`). Tests that measure real handshakes against bounded
//! deadlines warm the platform stack first with a separate connector and
//! socket so the application pool stays cold. This does not assert that a
//! cold production request meets its timeout.
#[cfg(target_os = "macos")]
pub(crate) fn warm_secure_transport_once() {
    use std::net::TcpListener as StdListener;
    static WARMUP: std::sync::Once = std::sync::Once::new();
    WARMUP.call_once(|| {
        let connector = native_tls::TlsConnector::new().expect("warmup connector");
        let listener = StdListener::bind("127.0.0.1:0").expect("bind warmup listener");
        let address = listener.local_addr().expect("warmup address");
        listener
            .set_nonblocking(true)
            .expect("nonblocking warmup listener");
        let server = std::thread::spawn(move || -> std::io::Result<()> {
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_secs(/*secs*/ 5);
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        drop(stream);
                        return Ok(());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if std::time::Instant::now() >= deadline {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "warmup listener did not receive a connection",
                            ));
                        }
                        std::thread::sleep(std::time::Duration::from_millis(/*millis*/ 10));
                    }
                    Err(error) => return Err(error),
                }
            }
        });
        let warmup = (|| -> std::io::Result<()> {
            let io_timeout = std::time::Duration::from_secs(/*secs*/ 2);
            let stream = std::net::TcpStream::connect_timeout(&address, io_timeout)?;
            stream.set_read_timeout(Some(io_timeout))?;
            stream.set_write_timeout(Some(io_timeout))?;
            // A closed peer suffices to create the platform TLS context. Socket
            // I/O is bounded; the one-time Security initialization runs here.
            drop(connector.connect("localhost", stream));
            Ok(())
        })();
        let accepted = server.join().expect("warmup server should not panic");
        warmup.expect("warmup socket setup should succeed");
        accepted.expect("warmup server should accept the local connection");
    });
}
