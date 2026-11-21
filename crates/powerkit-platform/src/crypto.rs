//! Cryptography from Windows (CNG): random bytes, HMAC-SHA256, PBKDF2 and AES-256-GCM.

use crate::{Context, Error, Result};
use windows::Win32::Security::Cryptography::{
    BCryptDecrypt, BCryptDeriveKeyPBKDF2, BCryptDestroyKey, BCryptEncrypt, BCryptGenRandom, BCryptGenerateSymmetricKey,
    BCryptHash, BCRYPT_AES_GCM_ALG_HANDLE, BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO, BCRYPT_FLAGS,
    BCRYPT_HMAC_SHA256_ALG_HANDLE, BCRYPT_KEY_HANDLE, BCRYPT_SHA256_ALG_HANDLE, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
};

pub const TAG_LEN: usize = 16;
pub const NONCE_LEN: usize = 12;

pub fn random(buf: &mut [u8]) -> Result<()> {
    // SAFETY: `buf` is a valid output slice.
    unsafe { BCryptGenRandom(None, buf, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }
        .ok()
        .context("BCryptGenRandom")
}

pub fn sha256(data: &[u8]) -> Result<[u8; 32]> {
    let mut out = [0u8; 32];
    // SAFETY: the slices are valid; the output is the size of a SHA-256 digest.
    unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out) }
        .ok()
        .context("SHA-256")?;
    Ok(out)
}

pub fn hmac_sha256(key: &[u8], data: &[u8]) -> Result<[u8; 32]> {
    let mut out = [0u8; 32];
    // SAFETY: the slices are valid; the output is the size of a SHA-256 digest.
    unsafe { BCryptHash(BCRYPT_HMAC_SHA256_ALG_HANDLE, Some(key), data, &mut out) }
        .ok()
        .context("HMAC-SHA256")?;
    Ok(out)
}

pub fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u64, out: &mut [u8]) -> Result<()> {
    // SAFETY: the slices are valid for the call.
    unsafe {
        BCryptDeriveKeyPBKDF2(
            BCRYPT_HMAC_SHA256_ALG_HANDLE,
            Some(password),
            Some(salt),
            iterations,
            out,
            0,
        )
    }
    .ok()
    .context("PBKDF2")
}

/// An AES-256-GCM key. Every nonce must be used once only with the same key.
pub struct Aead(BCRYPT_KEY_HANDLE);

// SAFETY: CNG key handles may be used from any thread, one call at a time, which
// `&mut self` on every use guarantees.
unsafe impl Send for Aead {}

impl Aead {
    pub fn new(key: &[u8; 32]) -> Result<Aead> {
        let mut handle = BCRYPT_KEY_HANDLE::default();
        // SAFETY: `handle` is a valid out pointer; CNG allocates the key object itself.
        unsafe { BCryptGenerateSymmetricKey(BCRYPT_AES_GCM_ALG_HANDLE, &mut handle, None, key, 0) }
            .ok()
            .context("AES-GCM key")?;
        Ok(Aead(handle))
    }

    fn mode(nonce: &[u8; NONCE_LEN], aad: &[u8], tag: &mut [u8; TAG_LEN]) -> BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO {
        BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO {
            cbSize: std::mem::size_of::<BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO>() as u32,
            dwInfoVersion: 1,
            pbNonce: nonce.as_ptr().cast_mut(),
            cbNonce: NONCE_LEN as u32,
            pbAuthData: if aad.is_empty() {
                std::ptr::null_mut()
            } else {
                aad.as_ptr().cast_mut()
            },
            cbAuthData: aad.len() as u32,
            pbTag: tag.as_mut_ptr(),
            cbTag: TAG_LEN as u32,
            ..Default::default()
        }
    }

    /// The ciphertext followed by the tag.
    pub fn seal(&mut self, nonce: &[u8; NONCE_LEN], aad: &[u8], plain: &[u8]) -> Result<Vec<u8>> {
        let mut tag = [0u8; TAG_LEN];
        let mode = Self::mode(nonce, aad, &mut tag);
        let mut out = vec![0u8; plain.len() + TAG_LEN];
        let mut written = 0u32;
        // SAFETY: `mode` points at `nonce`, `aad` and `tag`, which outlive the call;
        // the output has room for the whole plaintext.
        unsafe {
            BCryptEncrypt(
                self.0,
                Some(plain),
                Some((&raw const mode).cast()),
                None,
                Some(&mut out[..plain.len()]),
                &mut written,
                BCRYPT_FLAGS(0),
            )
        }
        .ok()
        .context("AES-GCM encrypt")?;
        out[plain.len()..].copy_from_slice(&tag);
        Ok(out)
    }

    /// The plaintext, or an error when the data or its tag was changed or the key
    /// differs.
    pub fn open(&mut self, nonce: &[u8; NONCE_LEN], aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>> {
        let Some(len) = sealed.len().checked_sub(TAG_LEN) else {
            return Err(Error::Other("sealed data shorter than its tag".into()));
        };
        let mut tag: [u8; TAG_LEN] = sealed[len..].try_into().expect("TAG_LEN bytes");
        let mode = Self::mode(nonce, aad, &mut tag);
        let mut out = vec![0u8; len];
        let mut written = 0u32;
        // SAFETY: as in `seal`.
        unsafe {
            BCryptDecrypt(
                self.0,
                Some(&sealed[..len]),
                Some((&raw const mode).cast()),
                None,
                Some(&mut out),
                &mut written,
                BCRYPT_FLAGS(0),
            )
        }
        .ok()
        .context("AES-GCM decrypt")?;
        Ok(out)
    }
}

impl Drop for Aead {
    fn drop(&mut self) {
        // SAFETY: the key was created in `new` and is destroyed once.
        let _ = unsafe { BCryptDestroyKey(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn sha256_matches_the_standard_vector() {
        assert_eq!(
            hex(&sha256(b"abc").unwrap()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn hmac_and_pbkdf2_match_the_published_vectors() {
        // RFC 4231, test case 2.
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?").unwrap()),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        let mut out = [0u8; 32];
        pbkdf2_sha256(b"password", b"salt", 2, &mut out).unwrap();
        assert_eq!(
            hex(&out),
            "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43"
        );
    }

    #[test]
    fn aes_gcm_matches_the_published_vector_and_rejects_changes() {
        // GCM specification, test case 14: zero key, zero nonce, one zero block.
        let mut aead = Aead::new(&[0; 32]).unwrap();
        let nonce = [0u8; NONCE_LEN];
        let sealed = aead.seal(&nonce, &[], &[0; 16]).unwrap();
        assert_eq!(
            hex(&sealed),
            "cea7403d4d606b6e074ec5d3baf39d18d0d1c8a799996bf0265b98b5d48ab919"
        );
        assert_eq!(aead.open(&nonce, &[], &sealed).unwrap(), vec![0; 16]);
        let mut changed = sealed.clone();
        changed[3] ^= 1;
        assert!(aead.open(&nonce, &[], &changed).is_err());
        assert!(aead.open(&nonce, b"other", &sealed).is_err());
        assert!(Aead::new(&[1; 32]).unwrap().open(&nonce, &[], &sealed).is_err());

        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        random(&mut a).unwrap();
        random(&mut b).unwrap();
        assert_ne!(a, b);
    }
}
