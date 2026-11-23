//! Bit-for-bit parity tests for `legion_shared::desktop_app_control`.

use legion_shared::desktop_app_control::{
    DesktopAppControlAddress, DesktopControlPlatform, resolve_desktop_app_control_address,
};

#[test]
fn desktop_app_control_addresses_match_node_across_platforms() {
    assert_eq!(
        resolve_desktop_app_control_address(
            r"C:\Users\alice\.legioncode\userdata",
            DesktopControlPlatform::Win32,
            r"C:\Temp",
            Some(1000),
        ),
        DesktopAppControlAddress {
            address: r"\\.\pipe\legioncode-app-e3761eff331ff5632fe52659".to_owned(),
            directory: None,
        }
    );

    assert_eq!(
        resolve_desktop_app_control_address(
            "/home/alice/.legioncode/userdata",
            DesktopControlPlatform::Linux,
            "/run/user/1000",
            Some(1000),
        ),
        DesktopAppControlAddress {
            address: "/run/user/1000/legioncode-1000/702fb4e231dbd415890acdaf.sock".to_owned(),
            directory: Some("/run/user/1000/legioncode-1000".to_owned()),
        }
    );

    assert_eq!(
        resolve_desktop_app_control_address(
            "/home/alice/.legioncode/userdata",
            DesktopControlPlatform::Linux,
            "/very/long/temporary/directory/path/that/exceeds/the/unix/domain/socket/maximum/byte/limit/easily",
            None,
        ),
        DesktopAppControlAddress {
            address: "/tmp/legioncode-702fb4e231db/702fb4e231dbd415890acdaf.sock".to_owned(),
            directory: Some("/tmp/legioncode-702fb4e231db".to_owned()),
        }
    );

    assert_eq!(
        resolve_desktop_app_control_address(
            "/Users/alice/.legioncode/userdata",
            DesktopControlPlatform::Darwin,
            "/var/folders/12/abcdef/T/",
            Some(501),
        ),
        DesktopAppControlAddress {
            address: "/var/folders/12/abcdef/T/legioncode-501/4f141606813d93b9d62afbfd.sock"
                .to_owned(),
            directory: Some("/var/folders/12/abcdef/T/legioncode-501".to_owned()),
        }
    );

    assert_eq!(
        resolve_desktop_app_control_address(
            "/Users/alice/.legioncode/userdata",
            DesktopControlPlatform::Darwin,
            "/var/folders/12/abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdef/T/",
            Some(501),
        ),
        DesktopAppControlAddress {
            address: "/tmp/legioncode-501/4f141606813d93b9d62afbfd.sock".to_owned(),
            directory: Some("/tmp/legioncode-501".to_owned()),
        }
    );
}

#[test]
fn unix_socket_byte_length_boundaries_are_exact_107_and_103() {
    let dir_suffix_len = "/legioncode-1000/702fb4e231dbd415890acdaf.sock".len();
    let exact_linux_temp = format!("/{}", "a".repeat(107 - 1 - dir_suffix_len));
    let linux_fit = resolve_desktop_app_control_address(
        "/home/alice/.legioncode/userdata",
        DesktopControlPlatform::Linux,
        &exact_linux_temp,
        Some(1000),
    );
    assert_eq!(linux_fit.address.len(), 107);
    assert!(linux_fit.address.starts_with(&exact_linux_temp));

    let over_linux_temp = format!("{exact_linux_temp}x");
    let linux_fallback = resolve_desktop_app_control_address(
        "/home/alice/.legioncode/userdata",
        DesktopControlPlatform::Linux,
        &over_linux_temp,
        Some(1000),
    );
    assert_eq!(
        linux_fallback.directory.as_deref(),
        Some("/tmp/legioncode-1000")
    );

    let exact_darwin_temp = format!("/{}", "a".repeat(103 - 1 - dir_suffix_len));
    let darwin_fit = resolve_desktop_app_control_address(
        "/home/alice/.legioncode/userdata",
        DesktopControlPlatform::Darwin,
        &exact_darwin_temp,
        Some(1000),
    );
    assert_eq!(darwin_fit.address.len(), 103);
    assert!(darwin_fit.address.starts_with(&exact_darwin_temp));

    let over_darwin_temp = format!("{exact_darwin_temp}x");
    let darwin_fallback = resolve_desktop_app_control_address(
        "/home/alice/.legioncode/userdata",
        DesktopControlPlatform::Darwin,
        &over_darwin_temp,
        Some(1000),
    );
    assert_eq!(
        darwin_fallback.directory.as_deref(),
        Some("/tmp/legioncode-1000")
    );
}
