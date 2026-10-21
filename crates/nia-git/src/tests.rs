use super::*;

#[test]
fn packet_lines_decode_control_frames_and_advertised_refs() {
    let id = "0123456789012345678901234567890123456789";
    let advertisement = format!(
        "001e# service=git-upload-pack\n0000{length:04x}{id} refs/heads/main\0side-band-64k\n0000",
        length = id.len() + " refs/heads/main\0side-band-64k\n".len() + 4,
    );
    let refs = parse_advertised_refs(advertisement.as_bytes()).unwrap();
    assert_eq!(refs[0].1, "refs/heads/main");
    assert_eq!(
        select_advertised_revision(&refs, "main").unwrap(),
        parse_object_id(id.as_bytes()).unwrap()
    );
}

#[test]
fn upload_pack_fetch_requests_depth_one() {
    let request = build_upload_pack_request(&[0x2a; 20]).unwrap();
    let packets = decode_packet_lines(&request).unwrap();
    assert_eq!(
        packets,
        vec![
            PacketLine::Data(
                format!("want {} ofs-delta side-band-64k\n", "2a".repeat(20)).into_bytes()
            ),
            PacketLine::Data(b"deepen 1\n".to_vec()),
            PacketLine::Flush,
            PacketLine::Data(b"done\n".to_vec()),
        ]
    );
}
