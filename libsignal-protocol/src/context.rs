use std::{
    convert::TryFrom,
    fmt::{self, Debug, Formatter},
    os::raw::{c_char, c_int, c_void},
    pin::Pin,
    ptr,
    rc::Rc,
    time::SystemTime,
};

use failure::Error;
use lock_api::RawMutex as _;
use log::Level;
use parking_lot::RawMutex;

use crate::{
    Address,
    Buffer,
    crypto::{Crypto, CryptoProvider},
    errors::{FromInternalErrorCode, InternalError},
    hkdf::HMACBasedKeyDerivationFunction,
    keys::{
        IdentityKeyPair, KeyPair, PreKeyList, PrivateKey, SessionSignedPreKey,
    },
    raw_ptr::Raw,
    session_builder::SessionBuilder, StoreContext, stores::{
        identity_key_store::{self as iks, IdentityKeyStore},
        pre_key_store::{self as pks, PreKeyStore},
        session_store::{self as sess, SessionStore},
        signed_pre_key_store::{self as spks, SignedPreKeyStore},
    },
};
#[cfg(feature = "crypto-native")]
use crate::crypto::DefaultCrypto;
#[cfg(feature = "crypto-openssl")]
use crate::crypto::OpenSSLCrypto;
// for rustdoc link resolution
#[allow(unused_imports)]
use crate::keys::{PreKey, PublicKey};

/// A helper function for generating a new [`IdentityKeyPair`].
pub fn generate_identity_key_pair(
    ctx: &Context,
) -> Result<IdentityKeyPair, Error> {
    unsafe {
        let mut key_pair = ptr::null_mut();
        sys::signal_protocol_key_helper_generate_identity_key_pair(
            &mut key_pair,
            ctx.raw(),
        )
        .into_result()?;
        Ok(IdentityKeyPair {
            raw: Raw::from_ptr(key_pair),
        })
    }
}

/// Generate a normal elliptic curve key pair.
pub fn generate_key_pair(ctx: &Context) -> Result<KeyPair, Error> {
    unsafe {
        let mut key_pair = ptr::null_mut();
        sys::curve_generate_key_pair(ctx.raw(), &mut key_pair).into_result()?;

        Ok(KeyPair {
            raw: Raw::from_ptr(key_pair),
        })
    }
}

/// Calculate the signature for a message.
///
/// # Examples
///
/// This is the counterpart to [`PublicKey::verify_signature`].
///
/// ```rust
/// # use libsignal_protocol::{keys::PublicKey, Context};
/// # use failure::Error;
/// # fn main() -> Result<(), Error> {
/// let ctx = Context::default();
/// let key_pair = libsignal_protocol::generate_key_pair(&ctx)?;
///
/// let msg = "Hello, World!";
/// let private_key = key_pair.private();
/// let signature = libsignal_protocol::calculate_signature(
///     &ctx,
///     &private_key,
///     msg.as_bytes(),
/// )?;
///
/// let public = key_pair.public();
/// let got = public.verify_signature(msg.as_bytes(), signature.as_slice());
/// assert!(got.is_ok());
/// # Ok(())
/// # }
/// ```
pub fn calculate_signature(
    ctx: &Context,
    private: &PrivateKey,
    message: &[u8],
) -> Result<Buffer, Error> {
    unsafe {
        let mut buffer = ptr::null_mut();
        sys::curve_calculate_signature(
            ctx.raw(),
            &mut buffer,
            private.raw.as_const_ptr(),
            message.as_ptr(),
            message.len(),
        )
        .into_result()?;

        Ok(Buffer::from_raw(buffer))
    }
}

/// Generate a new registration ID.
pub fn generate_registration_id(
    ctx: &Context,
    extended_range: i32,
) -> Result<u32, Error> {
    let mut id = 0;
    unsafe {
        sys::signal_protocol_key_helper_generate_registration_id(
            &mut id,
            extended_range,
            ctx.raw(),
        )
        .into_result()?;
    }

    Ok(id)
}

/// Generate a list of [`PreKey`]s. Clients should do this at install time, and
/// subsequently any time the list of [`PreKey`]s stored on the server runs low.
///
/// Pre key IDs are shorts, so they will eventually be repeated. Clients should
/// store pre keys in a circular buffer, so that they are repeated as
/// infrequently as possible.
pub fn generate_pre_keys(
    ctx: &Context,
    start: u32,
    count: u32,
) -> Result<PreKeyList, Error> {
    unsafe {
        let mut pre_keys_head = ptr::null_mut();
        sys::signal_protocol_key_helper_generate_pre_keys(
            &mut pre_keys_head,
            start,
            count,
            ctx.raw(),
        )
        .into_result()?;

        Ok(PreKeyList::from_raw(pre_keys_head))
    }
}

/// Generate a signed pre-key.
pub fn generate_signed_pre_key(
    ctx: &Context,
    identity_key_pair: &IdentityKeyPair,
    id: u32,
    timestamp: SystemTime,
) -> Result<SessionSignedPreKey, Error> {
    unsafe {
        let mut raw = ptr::null_mut();
        let unix_time = timestamp.duration_since(SystemTime::UNIX_EPOCH)?;

        sys::signal_protocol_key_helper_generate_signed_pre_key(
            &mut raw,
            identity_key_pair.raw.as_const_ptr(),
            id,
            unix_time.as_secs(),
            ctx.raw(),
        )
        .into_result()?;

        if raw.is_null() {
            Err(failure::err_msg("Unable to generate a signed pre key"))
        } else {
            Ok(SessionSignedPreKey {
                raw: Raw::from_ptr(raw),
            })
        }
    }
}

/// Create a container for the state used by the signal protocol.
pub fn store_context<P, K, S, I>(
    ctx: &Context,
    pre_key_store: P,
    signed_pre_key_store: K,
    session_store: S,
    identity_key_store: I,
) -> Result<StoreContext, Error>
where
    P: PreKeyStore + 'static,
    K: SignedPreKeyStore + 'static,
    S: SessionStore + 'static,
    I: IdentityKeyStore + 'static,
{
    unsafe {
        let mut store_ctx = ptr::null_mut();
        sys::signal_protocol_store_context_create(&mut store_ctx, ctx.raw())
            .into_result()?;

        let pre_key_store = pks::new_vtable(pre_key_store);
        sys::signal_protocol_store_context_set_pre_key_store(
            store_ctx,
            &pre_key_store,
        )
        .into_result()?;

        let signed_pre_key_store = spks::new_vtable(signed_pre_key_store);
        sys::signal_protocol_store_context_set_signed_pre_key_store(
            store_ctx,
            &signed_pre_key_store,
        )
        .into_result()?;

        let session_store = sess::new_vtable(session_store);
        sys::signal_protocol_store_context_set_session_store(
            store_ctx,
            &session_store,
        )
        .into_result()?;

        let identity_key_store = iks::new_vtable(identity_key_store);
        sys::signal_protocol_store_context_set_identity_key_store(
            store_ctx,
            &identity_key_store,
        )
        .into_result()?;

        Ok(StoreContext::new(store_ctx, &ctx.0))
    }
}

/// Create a new HMAC-based key derivation function.
pub fn create_hkdf(
    ctx: &Context,
    version: i32,
) -> Result<HMACBasedKeyDerivationFunction, Error> {
    HMACBasedKeyDerivationFunction::new(version, ctx)
}

/// Create a new session builder for communication with the user with the
/// specified address.
pub fn session_builder(
    ctx: &Context,
    store_context: &StoreContext,
    address: &Address,
) -> SessionBuilder {
    SessionBuilder::new(ctx, store_context, address)
}

/// Global state and callbacks used by the library.
///
/// Most functions which require access to the global context (e.g. for crypto
/// functions or locking) will accept a `&Context` as their first argument.
#[derive(Debug, Clone)]
pub struct Context(pub(crate) Rc<ContextInner>);

impl Context {
    /// Create a new [`Context`] using the provided cryptographic functions.
    pub fn new<C: Crypto + 'static>(crypto: C) -> Result<Context, Error> {
        ContextInner::new(crypto)
            .map(|c| Context(Rc::new(c)))
            .map_err(Error::from)
    }

    /// Access the original [`Crypto`] object.
    pub fn crypto(&self) -> &dyn Crypto { self.0.crypto.state() }

    pub(crate) fn raw(&self) -> *mut sys::signal_context { self.0.raw() }
}

#[cfg(feature = "crypto-native")]
impl Default for Context {
    fn default() -> Context {
        match Context::new(DefaultCrypto::default()) {
            Ok(c) => c,
            Err(e) => {
                panic!("Unable to create a context using the defaults: {}", e)
            },
        }
    }
}

#[cfg(feature = "crypto-openssl")]
impl Default for Context {
    fn default() -> Context {
        match Context::new(OpenSSLCrypto::default()) {
            Ok(c) => c,
            Err(e) => {
                panic!("Unable to create a context using the defaults: {}", e)
            },
        }
    }
}

/// Our Rust wrapper around the [`sys::signal_context`].
///
/// # Safety
///
/// This **must** outlive any data created by the `libsignal-protocol-c`
/// library. You'll usually do this by adding a `Rc<ContextInner>` to any
/// wrapper types.
#[allow(dead_code)]
pub(crate) struct ContextInner {
    raw: *mut sys::signal_context,
    crypto: CryptoProvider,
    // A pointer to our [`State`] has been passed to `libsignal-protocol-c`, so
    // we need to make sure it is never moved.
    state: Pin<Box<State>>,
}

impl ContextInner {
    pub(crate) fn new<C: Crypto + 'static>(
        crypto: C,
    ) -> Result<ContextInner, InternalError> {
        unsafe {
            let mut global_context: *mut sys::signal_context = ptr::null_mut();
            let crypto = CryptoProvider::new(crypto);
            let mut state = Pin::new(Box::new(State {
                mux: RawMutex::INIT,
                log_func: Box::new(default_log_func),
            }));

            let user_data =
                state.as_mut().get_mut() as *mut State as *mut c_void;
            sys::signal_context_create(&mut global_context, user_data)
                .into_result()?;
            sys::signal_context_set_crypto_provider(
                global_context,
                &crypto.vtable,
            )
            .into_result()?;
            sys::signal_context_set_locking_functions(
                global_context,
                Some(lock_function),
                Some(unlock_function),
            )
            .into_result()?;
            sys::signal_context_set_log_function(
                global_context,
                Some(log_trampoline),
            )
            .into_result()?;

            Ok(ContextInner {
                raw: global_context,
                crypto,
                state,
            })
        }
    }

    pub(crate) const fn raw(&self) -> *mut sys::signal_context { self.raw }
}

impl Drop for ContextInner {
    fn drop(&mut self) {
        unsafe {
            sys::signal_context_destroy(self.raw());
        }
    }
}

impl Debug for ContextInner {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ContextInner").finish()
    }
}

fn default_log_func(level: Level, message: &str) {
    log::log!(level, "{}", message);

    if level == Level::Error && std::env::var("RUST_BACKTRACE").is_ok() {
        log::error!("{}", failure::Backtrace::new());
    }
}

unsafe extern "C" fn log_trampoline(
    level: c_int,
    msg: *const c_char,
    len: usize,
    user_data: *mut c_void,
) {
    assert!(!msg.is_null());
    assert!(!user_data.is_null());

    let state = &*(user_data as *const State);
    let buffer = std::slice::from_raw_parts(msg as *const u8, len);
    let level = translate_log_level(level);

    if let Ok(message) = std::str::from_utf8(buffer) {
        (state.log_func)(level, message);
    }
}

fn translate_log_level(raw: c_int) -> Level {
    match u32::try_from(raw) {
        Ok(sys::SG_LOG_ERROR) => Level::Error,
        Ok(sys::SG_LOG_WARNING) => Level::Warn,
        Ok(sys::SG_LOG_INFO) => Level::Info,
        Ok(sys::SG_LOG_DEBUG) => Level::Debug,
        Ok(sys::SG_LOG_NOTICE) => Level::Trace,
        _ => Level::Info,
    }
}

unsafe extern "C" fn lock_function(user_data: *mut c_void) {
    let state = &*(user_data as *const State);
    state.mux.lock();
}

unsafe extern "C" fn unlock_function(user_data: *mut c_void) {
    let state = &*(user_data as *const State);
    state.mux.unlock();
}

/// The "user state" we pass to `libsignal-protocol-c` as part of the global
/// context.
///
/// # Safety
///
/// A pointer to this [`State`] will be shared throughout the
/// `libsignal-protocol-c` library, so any mutation **must** be done using the
/// appropriate synchronisation mechanisms (i.e. `RefCell` or atomics).
struct State {
    mux: RawMutex,
    log_func: Box<dyn Fn(Level, &str)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_initialization_example_from_readme() {
        let ctx = Context::default();

        drop(ctx);
    }
}
