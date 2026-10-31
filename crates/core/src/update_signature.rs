//! Streaming release verification, also used by the native packaging gate.
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::{io::Read, path::Path};
pub fn verify(file: &Path, signature: &str, public_key: &str, version: &str) -> Result<()> {
    let (key, signature) = checked_signature(signature, public_key, version)?;
    let mut verifier = key.verify_stream(&signature)?;
    let mut input = std::fs::File::open(file)?;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        verifier.update(&buffer[..n]);
    }
    verifier.finalize()?;
    Ok(())
}
pub fn verify_bytes(bytes: &[u8], signature: &str, public_key: &str, version: &str) -> Result<()> {
    let (key, signature) = checked_signature(signature, public_key, version)?;
    let mut verifier = key.verify_stream(&signature)?;
    verifier.update(bytes);
    verifier.finalize()?;
    Ok(())
}
fn checked_signature(
    signature: &str,
    public_key: &str,
    version: &str,
) -> Result<(minisign_verify::PublicKey, minisign_verify::Signature)> {
    let key_text = String::from_utf8(STANDARD.decode(public_key.trim())?)?;
    let signature_text = String::from_utf8(STANDARD.decode(signature.trim())?)?;
    let key = minisign_verify::PublicKey::decode(&key_text)?;
    let signature = minisign_verify::Signature::decode(&signature_text)?;
    let signed_version = signature
        .trusted_comment()
        .split('\t')
        .find_map(|field| field.strip_prefix("version:"))
        .context("Update signature has no signed version")?;
    if signed_version != version {
        bail!("Update signature version mismatch");
    }
    Ok((key, signature))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn verifies_bytes_and_binds_the_signed_version() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/update-signature");
        let signature = std::fs::read_to_string(fixture.join("payload.sig")).unwrap();
        let key = std::fs::read_to_string(fixture.join("public.key")).unwrap();
        verify(&fixture.join("payload"), &signature, &key, "1.2.3").unwrap();
        assert!(verify(&fixture.join("payload"), &signature, &key, "1.2.4").is_err());
        let temp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(temp.path(), b"tampered payload").unwrap();
        assert!(verify(temp.path(), &signature, &key, "1.2.3").is_err());
        assert!(verify(&fixture.join("payload"), "malformed", &key, "1.2.3").is_err());
    }
}
