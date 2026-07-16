use sha2::{Sha256, Digest};
use std::io::{self, Read, Write};
use std::path::Path;

/// Computes SHA-256 hash of bytes and returns it as a string in the format `sha256:<lowercase-hex>`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let hash = hasher.finalize();
    format!("sha256:{}", hex::encode(hash))
}

/// Computes SHA-256 hash of a file's contents using streaming with 8KB buffer.
/// Returns the hash as a string in the format `sha256:<lowercase-hex>`.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let file = std::fs::File::open(path)?;
    let mut reader = io::BufReader::with_capacity(8192, file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];

    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    let hash = hasher.finalize();
    Ok(format!("sha256:{}", hex::encode(hash)))
}

/// A wrapper around a writer that computes SHA-256 hash as bytes pass through.
/// Hashes all bytes written and provides `finish()` to retrieve the hash and original writer.
pub struct HashingWriter<W: Write> {
    writer: W,
    hasher: Sha256,
}

impl<W: Write> HashingWriter<W> {
    pub fn new(writer: W) -> Self {
        HashingWriter {
            writer,
            hasher: Sha256::new(),
        }
    }

    /// Finish hashing and return the original writer and the computed hash.
    pub fn finish(self) -> (W, String) {
        let hash = self.hasher.finalize();
        let hash_string = format!("sha256:{}", hex::encode(hash));
        (self.writer, hash_string)
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.hasher.update(buf);
        self.writer.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_sha256_empty() {
        let hash = sha256_hex(b"");
        assert_eq!(
            hash,
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_sha256_abc() {
        let hash = sha256_hex(b"abc");
        assert_eq!(
            hash,
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn test_sha256_file_matches_hex() {
        let data = b"abc";
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(data).unwrap();
        temp_file.flush().unwrap();

        let hex_hash = sha256_hex(data);
        let file_hash = sha256_file(temp_file.path()).unwrap();
        assert_eq!(hex_hash, file_hash);
    }

    #[test]
    fn test_hashing_writer_matches_hex() {
        let data = b"abc";

        // Create a vec writer and wrap it with HashingWriter
        let vec_writer = Vec::new();
        let mut hashing_writer = HashingWriter::new(vec_writer);
        hashing_writer.write_all(data).unwrap();
        hashing_writer.flush().unwrap();

        // Finish and get the hash
        let (_written_data, hash) = hashing_writer.finish();

        let expected_hash = sha256_hex(data);
        assert_eq!(hash, expected_hash);
    }

    #[test]
    fn test_sha256_file_empty() {
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.flush().unwrap();

        let file_hash = sha256_file(temp_file.path()).unwrap();
        assert_eq!(
            file_hash,
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
