//! The files of the bench self-test payload that kyto holds: the kernel
//! image is the bytes that its source gives, and the SHA-256 of the image
//! and of `config.txt` are those recorded here and in the README, which a
//! bench pins.

use kyto_bench_core::digest::Sha256;

/// The SHA-256 of `kernel_2712.img`. A change of the kernel's source that
/// changes its bytes changes this digest, the image and the README in the
/// same change, and every bench pins the new image again.
const KERNEL_SHA256: &str = "ad5c2ffab80d0d531acace80f606eca2330a67410b549b8556313c83959b1030";

/// The SHA-256 of `config.txt`.
const CONFIG_SHA256: &str = "221e6f20771e01705b21758eddce44d06c5ec6252db3dbb7e55e609a7a15ec39";

const README: &str = include_str!("../README.md");

#[test]
fn the_kernel_image_is_regenerated_from_its_source() {
    let image = kyto_self_test::kernel();
    assert_eq!(Sha256::of(&image).as_str(), KERNEL_SHA256);
    assert_eq!(
        image,
        include_bytes!("../kernel_2712.img"),
        "kernel_2712.img is the image that the source gives"
    );
}

#[test]
fn config_txt_holds_one_setting() {
    let config = include_bytes!("../config.txt");
    assert_eq!(Sha256::of(config).as_str(), CONFIG_SHA256);
    assert_eq!(config, b"uart_2ndstage=1\n");
}

#[test]
fn the_readme_records_the_size_and_digest_of_each_file() {
    let files: [(&str, &[u8], &str); 2] = [
        (
            "kernel_2712.img",
            include_bytes!("../kernel_2712.img"),
            KERNEL_SHA256,
        ),
        ("config.txt", include_bytes!("../config.txt"), CONFIG_SHA256),
    ];
    for (file, bytes, digest) in files {
        let row = format!("| `{file}` | {} | `{digest}` |", bytes.len());
        assert!(README.contains(&row), "the README has the row {row}");
    }
}
