use super::*;

pub(super) const METHOD: &[u8] = b"\x06\x04\x05\x06\x57\x27\x26\xd6\x10\x07\x00\x00\x23\xd1\xa1\x70\x70\x92\x02\xd7\xd0\x30\xd3\x11\x35\x00\x00\x10\x06\x00\x00\xd6\x91\x1c\x57\xd0\xc0\xd0\x46\xf4\x05\x00\x11\x22\x00\x00\xd2\x12\x17\x00\x00\xd1\x5d\xf2\x0a\x2c\xec\x25\x2c\xb8\x08\xa0\x4a\xf2\x0a\x01\x4f\x06\x01\xd3\x11\x06\x00\x00\x47\xd3\x11\x0b\x00\x00\xd1\x5d\xda\x14\x4a\xda\x14\x00\x4f\x06\x01\x47\x00\x00";
const CACHE: &[u8] = b"\x05\x03\x05\x06\x21\x27\x26\xd5\x10\x06\x00\x00\xab\xa0\xc5\x21\xaa\x28\xd6\xd0\x30\xd1\x12\x0b\x00\x00\x60\xfb\x0a\x2c\xd1\x02\x20\x4f\xa1\x0c\x02\x47\x00\x00";
const LABEL: &[u8] = b"\x2c\xd4\x22\x2c\xa4\x09\x49\x03";

pub(super) fn patch(data: &mut [u8], url: u32, label: u32) -> Result<()> {
    let mut positions = Vec::new();
    for (name, expected) in [
        ("WebAccount.showMoneyManagement", METHOD),
        ("WebAccount.cacheOffers", CACHE),
        ("MoneyChanger panel", LABEL),
    ] {
        let found = data
            .windows(expected.len())
            .enumerate()
            .filter_map(|(i, v)| (v == expected).then_some(i))
            .collect::<Vec<_>>();
        ensure!(
            found.len() == 1,
            "expected one exact {name}, found {}",
            found.len()
        );
        positions.push(found[0]);
    }
    let mut index = Vec::new();
    put_u30(&mut index, label);
    ensure!(index.len() == 2, "gold button string index width changed");
    // Bypass the abandoned payment overlay without enabling ExternalInterface or changing the reference asset.
    let mut code = b"\xd0\x30\x5d\x86\x08\x5d\xce\x01\x2c".to_vec();
    put_u30(&mut code, url);
    // Ruffle permits only new-tab navigation while script access remains disabled.
    code.extend(b"\x4a\xce\x01\x01\x2c\x92\x02\x4f\x86\x08\x02\x47");
    let start = positions[0] + 5;
    data[start..start + 87].fill(0x02);
    data[start..start + code.len()].copy_from_slice(&code);
    let start = positions[1] + 5;
    data[start..start + 33].fill(0x02);
    data[start] = 0x47;
    data[positions[2] + 4..positions[2] + 6].copy_from_slice(&index);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gold_navigation_is_exact_bounded_and_keeps_method_headers_and_layout() {
        let mut data = [METHOD, CACHE, LABEL].concat();
        let original = data.clone();
        patch(&mut data, 6000, 6001).unwrap();
        assert_eq!(data.len(), original.len());
        assert_eq!(&data[..5], &original[..5]);
        assert_eq!(&data[92..99], &original[92..99]);
        assert_eq!(&data[132..138], &original[132..138]);
        assert_eq!(&data[5..28], b"\xd0\x30\x5d\x86\x08\x5d\xce\x01\x2c\xf0\x2e\x4a\xce\x01\x01\x2c\x92\x02\x4f\x86\x08\x02\x47");
        assert!(data[28..92].iter().all(|v| *v == 0x02));
        assert_eq!(data[99], 0x47);
        assert!(data[100..132].iter().all(|v| *v == 0x02));
        assert!(patch(&mut data, 6000, 6001).is_err());
        let mut duplicate = original.repeat(2);
        assert!(patch(&mut duplicate, 6000, 6001).is_err());
        assert_eq!(duplicate, original.repeat(2));
        for index in 0..original.len() {
            let mut corrupt = original.clone();
            corrupt[index] ^= 1;
            let before = corrupt.clone();
            assert!(patch(&mut corrupt, 6000, 6001).is_err());
            assert_eq!(corrupt, before);
        }
    }
}
