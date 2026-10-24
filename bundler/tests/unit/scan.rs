//! Optional AU identity factory decoding.

use super::*;

/// Offers an old identity and skips a second unavailable one.
unsafe extern "C" fn count(_factory: *const sys::LegacyFactory, _plugin: u32) -> u32 {
    2
}

/// Supplies one complete identity in the wrapper's C layout.
unsafe extern "C" fn get(
    _factory: *const sys::LegacyFactory,
    _plugin: u32,
    index: u32,
    output: *mut sys::LegacyIdentity,
) -> bool {
    if index != 0 {
        return false;
    }
    unsafe {
        *output = sys::LegacyIdentity {
            type_: *b"aufx\0",
            subtype: *b"Scnt\0",
            manufacturer: *b"SwAu\0",
        }
    };
    true
}

/// An absent optional factory is harmless and successful identities are owned copies.
#[test]
fn optional_factory_decodes_retained_identity() {
    assert!(unsafe { read_legacy_ids(None, 0) }.unwrap().is_empty());
    let factory = sys::LegacyFactory {
        count: Some(count),
        get: Some(get),
    };
    let ids = unsafe { read_legacy_ids(Some(&factory), 0) }.unwrap();
    assert_eq!(
        ids,
        vec![AUv2Id {
            type_: *b"aufx",
            subtype: *b"Scnt",
            manufacturer: *b"SwAu"
        }]
    );
    let incomplete = sys::LegacyFactory {
        count: None,
        get: Some(get),
    };
    assert!(unsafe { read_legacy_ids(Some(&incomplete), 0) }.is_err());
}

/// Incomplete, unterminated, and non-ASCII identities are rejected.
#[test]
fn malformed_codes_are_rejected() {
    for code in [
        *b"au\0\0\0",
        *b"aufxx",
        [255, b'u', b'f', b'x', 0],
        *b"\nufx\0",
    ] {
        assert!(legacy_code(code).is_err());
    }
    assert_eq!(legacy_code(*b"Fx  \0").unwrap(), *b"Fx  ");
}
