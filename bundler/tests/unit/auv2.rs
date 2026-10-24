//! Additional AU identities retain their original plugin factory index.

use super::*;

/// Returns one Scintillate identity with the requested component type.
fn identity(type_: [u8; 4]) -> AUv2Id {
    AUv2Id {
        type_,
        subtype: *b"Scnt",
        manufacturer: *b"SwAu",
    }
}

/// Creates a scanned plugin with the old audio-effect identity.
fn library() -> PluginLibrary {
    PluginLibrary {
        plugins: vec![PluginInfo {
            clap_id: "net.sweet-audio.scintillate".into(),
            clap_name: "Scintillate".into(),
            clap_vendor: Some("Sweet Audio".into()),
            clap_version: Some("1.1.0".into()),
            clap_description: None,
            auv2_id: Some(identity(*b"aumf")),
            auv2_legacy_ids: vec![identity(*b"aufx")],
        }],
        has_auv2_entry: true,
        has_vst3_entry: false,
    }
}

/// Both identities stay visible and invoke the same factory entry point.
#[test]
fn additional_identity_shares_factory() {
    let library = library();
    let plist = info_plist_auv2(&library.plugins[0], &library, None).unwrap();
    assert_eq!(plist.matches("GetPluginFactoryAUV2_0").count(), 2);
    assert_eq!(
        plist
            .matches("<string>Sweet Audio: Scintillate</string>")
            .count(),
        2
    );
    assert!(plist.contains("<string>aumf</string>"));
    assert!(plist.contains("<string>aufx</string>"));
    assert!(!plist.contains("GetPluginFactoryAUV2_1"));
    assert!(!plist.contains("Unsearchable"));
}

/// Duplicate identities fail even when the primary ID comes from a CLI override.
#[test]
fn duplicate_identity_is_rejected() {
    let mut library = library();
    assert!(info_plist_auv2(&library.plugins[0], &library, Some(identity(*b"aufx"))).is_err());
    library.plugins[0].auv2_legacy_ids.push(identity(*b"aufx"));
    assert!(info_plist_auv2(&library.plugins[0], &library, None).is_err());
}
