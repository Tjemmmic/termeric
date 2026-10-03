#![no_main]
// The #[path] modules below are the app's full domain model and protocol;
// this target exercises only frame decoding and encoding.
#![allow(dead_code)]

use libfuzzer_sys::fuzz_target;

#[path = "../../termeric/src/domain/model.rs"]
mod model;
#[path = "../../termeric/src/domain/protocol.rs"]
mod protocol;

fuzz_target!(|data: &[u8]| {
    let payload = &data[..data.len().min(protocol::MAX_FRAME_BYTES)];
    let mut wire = Vec::with_capacity(payload.len() + 4);
    wire.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    wire.extend_from_slice(payload);

    if let Ok(frame) = protocol::read_frame::<protocol::ClientFrame>(&mut wire.as_slice()) {
        let mut encoded = Vec::new();
        protocol::write_frame(&mut encoded, &frame)
            .expect("a decoded client frame must remain serializable within the frame ceiling");
        let decoded = protocol::read_frame::<protocol::ClientFrame>(&mut encoded.as_slice())
            .expect("a reserialized client frame must remain decodable");
        assert_eq!(decoded, frame);
    }
    if let Ok(frame) = protocol::read_frame::<protocol::ServerFrame>(&mut wire.as_slice()) {
        let mut encoded = Vec::new();
        protocol::write_frame(&mut encoded, &frame)
            .expect("a decoded server frame must remain serializable within the frame ceiling");
        let decoded = protocol::read_frame::<protocol::ServerFrame>(&mut encoded.as_slice())
            .expect("a reserialized server frame must remain decodable");
        assert_eq!(decoded, frame);
    }
});
