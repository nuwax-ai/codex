use super::*;
use pretty_assertions::assert_eq;

#[test]
fn binary_input_without_credentials_is_clean() {
    let mut input = std::io::Cursor::new(vec![0xff; 32 * 1024]);
    assert!(!contains_secret(&mut input, &[b"secret-key"]).expect("scan binary data"));
}

#[test]
fn interrupted_reads_retry_and_other_read_errors_propagate() {
    struct InterruptedThenFailed {
        pending_interruption: bool,
    }
    impl Read for InterruptedThenFailed {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            if std::mem::take(&mut self.pending_interruption) {
                Err(std::io::ErrorKind::Interrupted.into())
            } else {
                Err(std::io::ErrorKind::PermissionDenied.into())
            }
        }
    }
    let error = contains_secret(
        &mut InterruptedThenFailed {
            pending_interruption: true,
        },
        &[b"secret-key"],
    )
    .expect_err("read failure must propagate after retrying interruption");
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
}
