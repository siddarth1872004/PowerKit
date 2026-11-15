//! Windows app packages without the Windows SDK tools: a self-signed signing
//! certificate, writing and signing an .msix, and registering it as a sparse package
//! whose files live in an ordinary folder. Windows 11 needs such a package before it
//! shows a program's entries at the top of the Explorer context menu.

use crate::util::{pcwstr, wide};
use crate::{Context, Error, Result};
use std::ffi::c_void;
use std::path::Path;
use windows::core::{Interface, HSTRING, PCSTR, PCWSTR, PSTR, PWSTR};
use windows::Foundation::Uri;
use windows::Management::Deployment::{AddPackageOptions, DeploymentResult, PackageManager};
use windows::Win32::Foundation::{GetLastError, HWND, SYSTEMTIME};
use windows::Win32::Security::Cryptography::*;
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
use windows::Win32::Storage::Packaging::Appx::{
    AppxFactory, IAppxFactory, APPX_COMPRESSION_OPTION_NORMAL, APPX_PACKAGE_SETTINGS,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CreateUri, Uri_CREATE_CANONICALIZE, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    STGM_CREATE, STGM_SHARE_EXCLUSIVE, STGM_WRITE,
};
use windows::Win32::System::SystemInformation::GetSystemTime;
use windows::Win32::UI::Shell::{SHCreateMemStream, SHCreateStreamOnFileEx};

const ENCODING: CERT_QUERY_ENCODING_TYPE = CERT_QUERY_ENCODING_TYPE(X509_ASN_ENCODING.0 | PKCS_7_ASN_ENCODING.0);
/// How long a new certificate is valid; a package signed with an expired one can no
/// longer be installed, though an installed one keeps working.
const VALID_YEARS: u16 = 25;

fn last_error(context: &str) -> Error {
    // SAFETY: no arguments.
    let code = unsafe { GetLastError() };
    Error::Os {
        context: context.into(),
        source: windows::core::Error::from(code.to_hresult()),
    }
}

struct Store(HCERTSTORE);

impl Store {
    /// A system certificate store such as `My` or `TrustedPeople`; `location` is
    /// `CERT_SYSTEM_STORE_CURRENT_USER` or `CERT_SYSTEM_STORE_LOCAL_MACHINE`.
    fn open(location: u32, name: &str, read_only: bool) -> Result<Store> {
        let wname = wide(name);
        let flags = location
            | if read_only {
                CERT_STORE_READONLY_FLAG.0 | CERT_STORE_OPEN_EXISTING_FLAG.0
            } else {
                0
            };
        // SAFETY: `wname` is NUL-terminated and outlives the call.
        let store = unsafe {
            CertOpenStore(
                CERT_STORE_PROV_SYSTEM_W,
                CERT_QUERY_ENCODING_TYPE(0),
                None,
                CERT_OPEN_STORE_FLAGS(flags),
                Some(wname.as_ptr() as *const c_void),
            )
        }
        .context(format!("opening the {name} certificate store"))?;
        Ok(Store(store))
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        // SAFETY: opened by `Store::open`, closed once.
        let _ = unsafe { CertCloseStore(Some(self.0), 0) };
    }
}

/// A certificate with its private key in the user's `My` store.
pub struct Certificate(*const CERT_CONTEXT);

impl Drop for Certificate {
    fn drop(&mut self) {
        // SAFETY: the context is owned by this value and freed once.
        let _ = unsafe { CertFreeCertificateContext(Some(self.0)) };
    }
}

// SAFETY: certificate contexts are reference-counted, immutable and thread-safe.
unsafe impl Send for Certificate {}

impl Certificate {
    /// The current code-signing certificate named `CN=<name>` in the user's store,
    /// made (with a new key) if there is none.
    pub fn find_or_create(name: &str) -> Result<Certificate> {
        let store = Store::open(CERT_SYSTEM_STORE_CURRENT_USER, "My", false)?;
        if let Some(c) = find(&store, name) {
            return Ok(c);
        }
        let created = create(name)?;
        let mut stored: *mut CERT_CONTEXT = std::ptr::null_mut();
        // SAFETY: the store is open; `created` is a valid context.
        unsafe {
            CertAddCertificateContextToStore(
                Some(store.0),
                created.0,
                CERT_STORE_ADD_REPLACE_EXISTING,
                Some(&mut stored),
            )
        }
        .context("saving the signing certificate")?;
        Ok(Certificate(stored))
    }

    /// The certificate's DER encoding, as a .cer file holds it.
    pub fn der(&self) -> Vec<u8> {
        // SAFETY: the context is valid; the encoded bytes live as long as it does.
        unsafe {
            let c = &*self.0;
            std::slice::from_raw_parts(c.pbCertEncoded, c.cbCertEncoded as usize).to_vec()
        }
    }

    /// Whether the machine trusts this certificate for app packages. Reading the
    /// machine store needs no elevation.
    pub fn trusted_by_machine(&self) -> Result<bool> {
        let store = Store::open(CERT_SYSTEM_STORE_LOCAL_MACHINE, "TrustedPeople", true)?;
        // SAFETY: the store is open; the found context is freed at once.
        unsafe {
            let found = CertFindCertificateInStore(
                store.0,
                ENCODING,
                0,
                CERT_FIND_EXISTING,
                Some(self.0 as *const c_void),
                None,
            );
            if found.is_null() {
                return Ok(false);
            }
            let _ = CertFreeCertificateContext(Some(found));
        }
        Ok(true)
    }
}

/// Add a DER-encoded certificate to the machine's trusted people, which Windows
/// requires of a self-signed package. Needs administrator rights.
pub fn trust_for_machine(der: &[u8]) -> Result<()> {
    let store = Store::open(CERT_SYSTEM_STORE_LOCAL_MACHINE, "TrustedPeople", false)?;
    // SAFETY: the store is open; `der` is a byte slice.
    unsafe {
        CertAddEncodedCertificateToStore(
            Some(store.0),
            X509_ASN_ENCODING,
            der,
            CERT_STORE_ADD_REPLACE_EXISTING,
            None,
        )
    }
    .context("adding the certificate to the machine's trusted people")
}

fn find(store: &Store, name: &str) -> Option<Certificate> {
    let wname = wide(name);
    let mut prev: *const CERT_CONTEXT = std::ptr::null();
    loop {
        // SAFETY: the store is open; `prev` is null or the last context found, which
        // this call frees.
        let c = unsafe {
            CertFindCertificateInStore(
                store.0,
                ENCODING,
                0,
                CERT_FIND_SUBJECT_STR,
                Some(wname.as_ptr() as *const c_void),
                Some(prev),
            )
        };
        if c.is_null() {
            return None;
        }
        prev = c;
        let mut size = 0u32;
        // SAFETY: `c` is a valid context; only the property's size is asked for.
        let has_key =
            unsafe { CertGetCertificateContextProperty(c, CERT_KEY_PROV_INFO_PROP_ID, None, &mut size) }.is_ok();
        // SAFETY: `c` is valid and its info pointer lives as long as it.
        let current = unsafe { CertVerifyTimeValidity(None, (*c).pCertInfo) } == 0;
        if has_key && current && subject_is(c, name) {
            // SAFETY: keeps a reference of its own; the search's one is freed by the
            // next search step or below.
            let own = unsafe { CertDuplicateCertificateContext(Some(c)) };
            // SAFETY: frees the search's reference.
            let _ = unsafe { CertFreeCertificateContext(Some(c)) };
            return Some(Certificate(own));
        }
    }
}

/// CERT_FIND_SUBJECT_STR matches any part of the subject, so check the whole of it.
fn subject_is(c: *const CERT_CONTEXT, name: &str) -> bool {
    let mut buf = vec![0u16; 512];
    // SAFETY: `c` is valid; `buf` is a writable buffer of the given length.
    let len = unsafe { CertGetNameStringW(c, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, Some(&mut buf)) } as usize;
    len > 0 && crate::util::from_wide(&buf) == name
}

fn encode(kind: PCSTR, info: *const c_void) -> Result<Vec<u8>> {
    let mut size = 0u32;
    // SAFETY: `info` points to the structure `kind` names; the first call sizes, the
    // second writes into a buffer of that size.
    unsafe {
        CryptEncodeObjectEx(
            X509_ASN_ENCODING,
            kind,
            info,
            CRYPT_ENCODE_OBJECT_FLAGS(0),
            None,
            None,
            &mut size,
        )
        .context("sizing a certificate extension")?;
        let mut out = vec![0u8; size as usize];
        CryptEncodeObjectEx(
            X509_ASN_ENCODING,
            kind,
            info,
            CRYPT_ENCODE_OBJECT_FLAGS(0),
            None,
            Some(out.as_mut_ptr() as *mut c_void),
            &mut size,
        )
        .context("encoding a certificate extension")?;
        out.truncate(size as usize);
        Ok(out)
    }
}

fn create(name: &str) -> Result<Certificate> {
    let container = wide(&format!("{name} signing key"));
    // SAFETY: handles are created and freed here; every buffer outlives the call
    // that reads it.
    unsafe {
        let mut provider = NCRYPT_PROV_HANDLE::default();
        NCryptOpenStorageProvider(&mut provider, MS_KEY_STORAGE_PROVIDER, 0)
            .context("opening the key storage provider")?;
        let mut key = NCRYPT_KEY_HANDLE::default();
        let made = NCryptCreatePersistedKey(
            provider,
            &mut key,
            NCRYPT_RSA_ALGORITHM,
            pcwstr(&container),
            CERT_KEY_SPEC(0),
            NCRYPT_OVERWRITE_KEY_FLAG,
        )
        .context("creating the signing key")
        .and_then(|()| {
            NCryptSetProperty(
                NCRYPT_HANDLE(key.0),
                NCRYPT_LENGTH_PROPERTY,
                &2048u32.to_le_bytes(),
                NCRYPT_FLAGS(0),
            )
            .context("setting the key length")
        })
        .and_then(|()| NCryptFinalizeKey(key, NCRYPT_FLAGS(0)).context("finishing the signing key"))
        .and_then(|()| self_sign(name, key, &container));
        if key.0 != 0 {
            let _ = NCryptFreeObject(NCRYPT_HANDLE(key.0));
        }
        let _ = NCryptFreeObject(NCRYPT_HANDLE(provider.0));
        made
    }
}

/// # Safety
/// `key` must be a finalized key named `container` in the software key provider.
unsafe fn self_sign(name: &str, key: NCRYPT_KEY_HANDLE, container: &[u16]) -> Result<Certificate> {
    let subject = wide(&format!("CN={name}"));
    let mut size = 0u32;
    // SAFETY: `subject` is NUL-terminated; first call sizes, second fills.
    let subject_blob = unsafe {
        CertStrToNameW(
            X509_ASN_ENCODING,
            pcwstr(&subject),
            CERT_X500_NAME_STR,
            None,
            None,
            &mut size,
            None,
        )
        .context("encoding the certificate subject")?;
        let mut blob = vec![0u8; size as usize];
        CertStrToNameW(
            X509_ASN_ENCODING,
            pcwstr(&subject),
            CERT_X500_NAME_STR,
            None,
            Some(blob.as_mut_ptr()),
            &mut size,
            None,
        )
        .context("encoding the certificate subject")?;
        blob.truncate(size as usize);
        blob
    };
    let subject_info = CRYPT_INTEGER_BLOB {
        cbData: subject_blob.len() as u32,
        pbData: subject_blob.as_ptr() as *mut u8,
    };

    let mut code_signing = PSTR(szOID_PKIX_KP_CODE_SIGNING.0 as *mut u8);
    let usage = CTL_USAGE {
        cUsageIdentifier: 1,
        rgpszUsageIdentifier: &mut code_signing,
    };
    let eku = encode(X509_ENHANCED_KEY_USAGE, &usage as *const _ as *const c_void)?;
    let mut digital_signature = CERT_DIGITAL_SIGNATURE_KEY_USAGE as u8;
    let key_usage_bits = CRYPT_BIT_BLOB {
        cbData: 1,
        pbData: &mut digital_signature,
        cUnusedBits: 0,
    };
    let key_usage = encode(X509_KEY_USAGE, &key_usage_bits as *const _ as *const c_void)?;
    let end_entity = CERT_BASIC_CONSTRAINTS2_INFO::default();
    let basic = encode(X509_BASIC_CONSTRAINTS2, &end_entity as *const _ as *const c_void)?;
    let ext = |oid: PCSTR, critical: bool, value: &Vec<u8>| CERT_EXTENSION {
        pszObjId: PSTR(oid.0 as *mut u8),
        fCritical: critical.into(),
        Value: CRYPT_INTEGER_BLOB {
            cbData: value.len() as u32,
            pbData: value.as_ptr() as *mut u8,
        },
    };
    let mut list = [
        ext(szOID_ENHANCED_KEY_USAGE, false, &eku),
        ext(szOID_KEY_USAGE, true, &key_usage),
        ext(szOID_BASIC_CONSTRAINTS2, true, &basic),
    ];
    let extensions = CERT_EXTENSIONS {
        cExtension: list.len() as u32,
        rgExtension: list.as_mut_ptr(),
    };

    let mut provider_name = wide(&String::from_utf16_lossy(
        // SAFETY: the constant is a NUL-terminated static string.
        unsafe { MS_KEY_STORAGE_PROVIDER.as_wide() },
    ));
    let mut container = container.to_vec();
    let key_info = CRYPT_KEY_PROV_INFO {
        pwszContainerName: PWSTR(container.as_mut_ptr()),
        pwszProvName: PWSTR(provider_name.as_mut_ptr()),
        ..Default::default()
    };
    let algorithm = CRYPT_ALGORITHM_IDENTIFIER {
        pszObjId: PSTR(szOID_RSA_SHA256RSA.0 as *mut u8),
        ..Default::default()
    };
    // SAFETY: no arguments.
    let mut end: SYSTEMTIME = unsafe { GetSystemTime() };
    end.wYear += VALID_YEARS;
    // 29 February does not exist in most target years.
    end.wDay = end.wDay.min(28);

    // SAFETY: every pointer refers to a local that outlives the call.
    let c = unsafe {
        CertCreateSelfSignCertificate(
            Some(HCRYPTPROV_OR_NCRYPT_KEY_HANDLE(key.0)),
            &subject_info,
            CERT_CREATE_SELFSIGN_FLAGS(0),
            Some(&key_info),
            Some(&algorithm),
            None,
            Some(&end),
            Some(&extensions),
        )
    };
    if c.is_null() {
        return Err(last_error("creating the signing certificate"));
    }
    Ok(Certificate(c))
}

/// Write an .msix holding `manifest` and `files` (name, MIME type, bytes).
pub fn write_package(path: &Path, manifest: &str, files: &[(&str, &str, &[u8])]) -> Result<()> {
    // SAFETY: the calls are made on interfaces created just before; all strings
    // outlive the calls that read them.
    unsafe {
        // Already initialised in a compatible way is fine; the calls below report any
        // real failure.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: IAppxFactory =
            CoCreateInstance(&AppxFactory, None, CLSCTX_INPROC_SERVER).context("starting the package writer")?;
        let wpath = wide(&path.to_string_lossy());
        let out = SHCreateStreamOnFileEx(
            pcwstr(&wpath),
            (STGM_CREATE | STGM_WRITE | STGM_SHARE_EXCLUSIVE).0,
            FILE_ATTRIBUTE_NORMAL.0,
            true,
            None,
        )
        .context(format!("creating {}", path.display()))?;
        let sha256 = CreateUri(
            windows::core::w!("http://www.w3.org/2001/04/xmlenc#sha256"),
            Uri_CREATE_CANONICALIZE,
            None,
        )
        .context("naming the hash method")?;
        let settings = APPX_PACKAGE_SETTINGS {
            forceZip32: true.into(),
            hashMethod: std::mem::ManuallyDrop::new(Some(sha256)),
        };
        let writer = factory.CreatePackageWriter(&out, &settings);
        drop(std::mem::ManuallyDrop::into_inner(settings.hashMethod));
        let writer = writer.context("starting the package")?;
        for (name, mime, bytes) in files {
            let stream = SHCreateMemStream(Some(bytes)).ok_or_else(|| Error::Other("out of memory".into()))?;
            let (wname, wmime) = (wide(name), wide(mime));
            writer
                .AddPayloadFile(pcwstr(&wname), pcwstr(&wmime), APPX_COMPRESSION_OPTION_NORMAL, &stream)
                .context(format!("adding {name} to the package"))?;
        }
        let manifest =
            SHCreateMemStream(Some(manifest.as_bytes())).ok_or_else(|| Error::Other("out of memory".into()))?;
        writer.Close(&manifest).context("finishing the package")
    }
}

/// `SIGNER_SIGN_EX2_PARAMS` from the Windows SDK, missing from the bindings.
#[repr(C)]
struct SignerSignEx2Params {
    flags: u32,
    subject: *const SIGNER_SUBJECT_INFO,
    cert: *const SIGNER_CERT,
    signature: *const SIGNER_SIGNATURE_INFO,
    provider: *const SIGNER_PROVIDER_INFO,
    timestamp_flags: u32,
    timestamp_oid: PCSTR,
    timestamp_url: PCWSTR,
    attributes: *const CRYPT_ATTRIBUTES,
    sip_data: *const c_void,
    context: *mut *mut SIGNER_CONTEXT,
    policy: *const CERT_STRONG_SIGN_PARA,
    reserved: *const c_void,
}

/// `APPX_SIP_CLIENT_DATA`: signing an app package needs the parameters passed a
/// second time, and hands back state that must be released.
#[repr(C)]
struct AppxSipClientData {
    params: *const SignerSignEx2Params,
    state: *mut c_void,
}

/// Sign the package at `path` with `cert`. The package's publisher must equal the
/// certificate's subject.
pub fn sign_package(path: &Path, cert: &Certificate) -> Result<()> {
    let wpath = wide(&path.to_string_lossy());
    let file = SIGNER_FILE_INFO {
        cbSize: size_of::<SIGNER_FILE_INFO>() as u32,
        pwszFileName: pcwstr(&wpath),
        ..Default::default()
    };
    let mut index = 0u32;
    let subject = SIGNER_SUBJECT_INFO {
        cbSize: size_of::<SIGNER_SUBJECT_INFO>() as u32,
        pdwIndex: &mut index,
        dwSubjectChoice: SIGNER_SUBJECT_FILE,
        Anonymous: SIGNER_SUBJECT_INFO_0 {
            pSignerFileInfo: &file as *const _ as *mut _,
        },
    };
    let mut store_info = SIGNER_CERT_STORE_INFO {
        cbSize: size_of::<SIGNER_CERT_STORE_INFO>() as u32,
        pSigningCert: cert.0,
        dwCertPolicy: SIGNER_CERT_POLICY_CHAIN,
        ..Default::default()
    };
    let signer = SIGNER_CERT {
        cbSize: size_of::<SIGNER_CERT>() as u32,
        dwCertChoice: SIGNER_CERT_STORE,
        Anonymous: SIGNER_CERT_0 {
            pCertStoreInfo: &mut store_info,
        },
        hwnd: HWND::default(),
    };
    let signature = SIGNER_SIGNATURE_INFO {
        cbSize: size_of::<SIGNER_SIGNATURE_INFO>() as u32,
        algidHash: CALG_SHA_256,
        dwAttrChoice: SIGNER_NO_ATTR,
        ..Default::default()
    };
    let mut context: *mut SIGNER_CONTEXT = std::ptr::null_mut();
    let mut params = SignerSignEx2Params {
        flags: 0,
        subject: &subject,
        cert: &signer,
        signature: &signature,
        provider: std::ptr::null(),
        timestamp_flags: 0,
        timestamp_oid: PCSTR::null(),
        timestamp_url: PCWSTR::null(),
        attributes: std::ptr::null(),
        sip_data: std::ptr::null(),
        context: &mut context,
        policy: std::ptr::null(),
        reserved: std::ptr::null(),
    };
    let mut sip = AppxSipClientData {
        params: &params,
        state: std::ptr::null_mut(),
    };
    params.sip_data = &mut sip as *mut _ as *const c_void;
    // SAFETY: every structure is a live local of the layout the SDK defines; the
    // returned SIP state and signer context are released below.
    unsafe {
        let signed = SignerSignEx2(
            SIGNER_SIGN_FLAGS(0),
            &subject,
            &signer,
            &signature,
            None,
            None,
            PCSTR::null(),
            PCWSTR::null(),
            None,
            Some(params.sip_data),
            &mut context,
            None,
            None,
        );
        if !sip.state.is_null() {
            drop(windows::core::IUnknown::from_raw(sip.state));
        }
        if !context.is_null() {
            let _ = SignerFreeSignerContext(context);
        }
        signed.context("signing the package")
    }
}

fn uri(path: &Path) -> Result<Uri> {
    Uri::CreateUri(&HSTRING::from(path.as_os_str())).context(format!("naming {}", path.display()))
}

fn deployed(outcome: windows::core::Result<DeploymentResult>, what: &str) -> Result<()> {
    let result = outcome.context(what)?;
    let code = result.ExtendedErrorCode().context(what)?;
    if code.is_err() {
        let text = result.ErrorText().map(|t| t.to_string()).unwrap_or_default();
        return Err(Error::Other(format!("{what}: {} {}", code.message(), text.trim())));
    }
    Ok(())
}

/// Register the signed sparse package at `msix` for the current user, with its
/// files in the folder `external`.
pub fn register_sparse(msix: &Path, external: &Path) -> Result<()> {
    let manager = PackageManager::new().context("starting the package manager")?;
    let options = AddPackageOptions::new().context("starting the package manager")?;
    options
        .SetExternalLocationUri(&uri(external)?)
        .context("setting the package folder")?;
    let operation = manager
        .AddPackageByUriAsync(&uri(msix)?, &options)
        .context("registering the package")?;
    deployed(operation.join(), "registering the package")
}

/// Full names of the current user's packages named `name` from `publisher`.
pub fn installed(name: &str, publisher: &str) -> Result<Vec<String>> {
    let manager = PackageManager::new().context("starting the package manager")?;
    let found = manager
        .FindPackagesByUserSecurityIdNamePublisher(&HSTRING::new(), &HSTRING::from(name), &HSTRING::from(publisher))
        .context("listing packages")?;
    let mut names = Vec::new();
    for p in found {
        names.push(
            p.Id()
                .and_then(|id| id.FullName())
                .context("reading a package name")?
                .to_string(),
        );
    }
    Ok(names)
}

/// Unregister the package `full_name` for the current user.
pub fn remove(full_name: &str) -> Result<()> {
    let manager = PackageManager::new().context("starting the package manager")?;
    let operation = manager
        .RemovePackageAsync(&HSTRING::from(full_name))
        .context("removing the package")?;
    deployed(operation.join(), "removing the package")
}
