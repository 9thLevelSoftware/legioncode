//! Bit-for-bit parity and edge-case tests for `legion_shared::qr_code`.

use legion_shared::qr_code::{
    QrCode, QrEcc, QrError, QrSegment, QrSegmentMode, build_pairing_app_link, build_pairing_url,
    render_terminal_qr_code,
};

fn dump_bits(qr: &QrCode) -> String {
    let size = i32::from(qr.size());
    let mut bits = String::with_capacity(usize::try_from(size * size).unwrap_or(0));
    for y in 0..size {
        for x in 0..size {
            bits.push(if qr.get_module(x, y) { '1' } else { '0' });
        }
    }
    bits
}

#[test]
fn golden_terminal_qr_code_matches_node_startup_access_output() -> Result<(), QrError> {
    let url = "http://127.0.0.1:3773/pair#token=test-token";
    let qr = QrCode::encode_text(url, QrEcc::Medium)?;
    assert_eq!(qr.version(), 4);
    assert_eq!(qr.error_correction_level(), QrEcc::Quartile);
    assert_eq!(qr.size(), 33);
    assert_eq!(qr.mask(), 2);

    let expected_terminal = [
        "                                     ",
        "  █▀▀▀▀▀█ ▀▄▀█▄█▄▄▀ █▀▄██▀█ █▀▀▀▀▀█  ",
        "  █ ███ █  ▀▄▄  ▄▀  ▄▀ ▀ █  █ ███ █  ",
        "  █ ▀▀▀ █ █ ▄▀▀ ▄▀▀█ ▄▀█ ▀  █ ▀▀▀ █  ",
        "  ▀▀▀▀▀▀▀ ▀▄█▄▀▄█▄█ ▀▄█▄▀▄█ ▀▀▀▀▀▀▀  ",
        "  ▄▀▀▀▀▀▀▀  ▀█  ██ █ █▀  ▄█▄▄█▀▄▄▄█  ",
        "   ▄ ▄▀▀▀▄▄▀▀  ▀█▄██▄▄███ ██  █▄██   ",
        "  ▀▄█ ▄█▀ █▄▀█▀▄▄▄ ▄▄   █▄ ▀▄▀▀▀▄▄█  ",
        "  ▀▄▀█▀▀▀▄ ▀▀▄██ ███ ▄▄▄▀  ▄ ▄█ ▄▀   ",
        "  ▀█ █▄▀▀▀▄▀██▄█▄ ▄▄██▀▄▄▄▄▄▄█▀▄▄ █  ",
        "  ██▀▀██▀▄ ▄█  ▀▀▀▄▀▄▄██▀▄▀▀█ █▄██   ",
        "  █▀▀█▀ ▀██▀▄▄▀▀ ▀▄▀▄▄▄▄▀█▀▄▄▀▀▀▄ ▄  ",
        "  █  ▄ ▄▀█▀███▀ █▄█▀█ ██▄ ▀ ▄ ██▄█▄  ",
        "  ▀ ▀   ▀ █  ▀▄ ▀ ▄▀ ▄▀ ▄ █▀▀▀█▀▄ █  ",
        "  █▀▀▀▀▀█ █ █▄█▀▄██▀█▄▀▄█▀█ ▀ █ █▀   ",
        "  █ ███ █ ██▄▄▄▀█▀█▄█▄▄ ▀▄▀▀▀███▄ ▄  ",
        "  █ ▀▀▀ █ █ ▀▀▄▄█▀▀▀▄▄▀▄ █▀███▄▄█    ",
        "  ▀▀▀▀▀▀▀  ▀ ▀ ▀ ▀ ▀ ▀▀  ▀▀▀ ▀▀  ▀   ",
        "                                     ",
    ]
    .join("\n");

    let rendered = render_terminal_qr_code(url, 2)?;
    assert_eq!(rendered, expected_terminal);
    Ok(())
}

#[test]
fn bit_matrix_matches_node_across_modes_and_ecc_boost() -> Result<(), QrError> {
    let empty = QrCode::encode_text("", QrEcc::Low)?;
    assert_eq!(empty.version(), 1);
    assert_eq!(empty.error_correction_level(), QrEcc::High);
    assert_eq!(empty.size(), 21);
    assert_eq!(empty.mask(), 6);
    assert_eq!(
        dump_bits(&empty),
        "111111100100001111111100000100011001000001101110101111101011101101110101110101011101101110100111001011101100000100101101000001111111101010101111111000000000000000000000000110110000100001100110001011001100111011110001111000111101001010110001111000110010101100101000001111111000000001100010000111111111101110011001101100000100111011000100101110101101001010110101110101100100110000101110100101100111011100000100000010101011111111100011101110110"
    );

    let numeric = QrCode::encode_text("0123456789012345", QrEcc::Medium)?;
    assert_eq!(numeric.version(), 1);
    assert_eq!(numeric.error_correction_level(), QrEcc::High);
    assert_eq!(numeric.size(), 21);
    assert_eq!(numeric.mask(), 1);
    assert_eq!(
        dump_bits(&numeric),
        "111111100011001111111100000101000101000001101110101001001011101101110101000101011101101110101100101011101100000101001101000001111111101010101111111000000000010000000000001001111011110111110001111001011100001000111100100001010110100111000010011000101000111010100110111011011000000001011000011000111111101100100101011100000101010110010100101110100011111001001101110100000101001000101110101101010011011100000100010001101110111111100011001111011"
    );

    let alnum = QrCode::encode_text("HELLO WORLD 12345 $%*+-./:", QrEcc::Quartile)?;
    assert_eq!(alnum.version(), 2);
    assert_eq!(alnum.error_correction_level(), QrEcc::Quartile);
    assert_eq!(alnum.size(), 25);
    assert_eq!(alnum.mask(), 3);
    assert_eq!(
        dump_bits(&alnum),
        "1111111000010110001111111100000101101111110100000110111010110011110010111011011101001101000101011101101110100011011110101110110000010011010111010000011111111010101010101111111000000000101011100000000001110110011001110000001101101100000011000110010101010100111110101100101101111100000110101101001001110010011000100001011010011010100000100110011000100101101111001110010100010011001110110000110111111001001000100110100111111011000000000100011101000110111111111000001101101010001100000101011011110001110010111010011001001111101111011101011010110000000001101110101010000110011001010000010111001011001010011111111001111010001100001"
    );

    let utf8 = QrCode::encode_text("héllo 🚀", QrEcc::High)?;
    assert_eq!(utf8.version(), 2);
    assert_eq!(utf8.error_correction_level(), QrEcc::High);
    assert_eq!(utf8.size(), 25);
    assert_eq!(utf8.mask(), 4);
    assert_eq!(
        dump_bits(&utf8),
        "1111111001110111001111111100000101001010110100000110111010010100001010111011011101001000011101011101101110100011011100101110110000010101000011010000011111111010101010101111111000000001010110000000000000001111000011010011000100011100101100000111100100011101100100110001111011100101101101111000111101011010111001010001000010101110101010011010010110110100101011011010001100011010001010000001110010000110111100101001101111111010000000000111100111000111001111111010011111101011100100000101010010110001010110111010110000001111101001011101001001101001001101101110100101101111011001010000010001101111010011101111111001111011100110111"
    );

    Ok(())
}

#[test]
fn version_11_multi_block_with_version_info_matches_node() -> Result<(), QrError> {
    let long_url = format!(
        "https://legion.codes/pair?host=tailnet&label=workstation#token={}",
        "a".repeat(156)
    );
    let qr = QrCode::encode_text(&long_url, QrEcc::Medium)?;
    assert_eq!(qr.version(), 11);
    assert_eq!(qr.error_correction_level(), QrEcc::Medium);
    assert_eq!(qr.size(), 61);
    assert_eq!(qr.mask(), 1);

    let bits = dump_bits(&qr);
    assert_eq!(bits.len(), 61 * 61);
    let expected_prefix = "11111110101110010010100111100010101010101010101011011011111111000001001010011100000001110101000100010001000100101101000001101110101100101111011111011000101001000100010111011110101110110111010001100000000100000001010001100110011010101101010111011011101000010111111011001000111110001000100010101011001011101100000101100101000000100100010001000000000000010001000100000111111110101010101010101010101010101010101010101010101";
    let expected_suffix = "10101001100100110100011111011101110111011101110010001111110000010011110100111111111001000100010001000100010010100010001111111011000000101000101110101010101010101010101011100010001";
    assert!(bits.starts_with(expected_prefix));
    assert!(bits.ends_with(expected_suffix));
    Ok(())
}

#[test]
fn pairing_url_and_app_link_match_node_urlsearchparams() -> Result<(), QrError> {
    assert_eq!(
        build_pairing_url("http://127.0.0.1:3773", "test-token")?,
        "http://127.0.0.1:3773/pair#token=test-token"
    );
    assert_eq!(
        build_pairing_url(
            "https://Example.COM:443/old?keep=1+2&token=old#frag",
            "a b/c:d~e!"
        )?,
        "https://example.com/pair?keep=1+2#token=a+b%2Fc%3Ad%7Ee%21"
    );
    assert_eq!(
        build_pairing_url("http://127.0.0.1:80/?&a&&token=1&b=c=d", "t+1")?,
        "http://127.0.0.1/pair?a=&b=c%3Dd#token=t%2B1"
    );
    assert_eq!(
        build_pairing_url("wss://[::1]:443/a?token=1", "")?,
        "wss://[::1]/pair#token="
    );
    assert_eq!(
        build_pairing_url("https://[::1]:8443/abc?x=1&token=old&y=2#old", "tok+1=2&3")?,
        "https://[::1]:8443/pair?x=1&y=2#token=tok%2B1%3D2%263"
    );
    assert_eq!(
        build_pairing_url(
            "  https://192.168.1.5:3773/anything?k=%E2%9C%93&sp=1+2%203#x  ",
            "tok"
        )?,
        "https://192.168.1.5:3773/pair?k=%E2%9C%93&sp=1+2+3#token=tok"
    );
    assert_eq!(
        build_pairing_app_link("http://127.0.0.1:3773/pair#token=test-token"),
        "legioncode://pair?pairingUrl=http%3A%2F%2F127.0.0.1%3A3773%2Fpair%23token%3Dtest-token"
    );
    assert_eq!(
        build_pairing_url("/relative/path", "tok"),
        Err(QrError::InvalidUrl)
    );
    assert_eq!(
        build_pairing_url("http://:3773", "tok"),
        Err(QrError::InvalidUrl)
    );
    Ok(())
}

#[test]
fn validates_qr_errors_segments_and_margins() -> Result<(), QrError> {
    assert_eq!(
        render_terminal_qr_code("hi", -1),
        Err(QrError::InvalidMargin(-1))
    );
    assert_eq!(
        QrCode::encode_binary(&[0u8; 4000], QrEcc::Low),
        Err(QrError::DataTooLong)
    );
    assert!(matches!(
        QrSegment::make_numeric("12a"),
        Err(QrError::InvalidValue(_))
    ));
    assert!(matches!(
        QrSegment::make_alphanumeric("abc"),
        Err(QrError::InvalidValue(_))
    ));

    let eci = QrSegment::make_eci(26)?;
    assert_eq!(eci.mode(), QrSegmentMode::Eci);
    assert_eq!(eci.num_chars(), 0);
    assert_eq!(eci.bit_len(), 8);

    let no_boost = QrCode::encode_segments_with_options(
        &QrSegment::make_segments("12345"),
        QrEcc::Low,
        1,
        40,
        Some(3),
        false,
    )?;
    assert_eq!(no_boost.error_correction_level(), QrEcc::Low);
    assert_eq!(no_boost.mask(), 3);
    assert!(!no_boost.get_module(-1, 0));
    assert!(!no_boost.get_module(0, 99));
    Ok(())
}
