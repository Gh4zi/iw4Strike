use master_protocol::{ContentFlags, ControlRequest, RequestBody, decode_request, encode_request};

fn create_room(dedicated: bool) -> ControlRequest {
    ControlRequest {
        request_id: 7,
        body: RequestBody::CreateRoom {
            name: "iw4Strike Local".into(),
            map: "mp_rust".into(),
            mode: "sd".into(),
            max_players: 10,
            requires: ContentFlags(1),
            available: ContentFlags(1),
            password: String::new(),
            dedicated,
        },
    }
}

#[test]
fn create_room_round_trips_with_and_without_the_dedicated_byte() {
    for dedicated in [false, true] {
        let request = create_room(dedicated);
        let bytes = encode_request(&request).expect("encodes");
        assert_eq!(decode_request(&bytes).expect("decodes"), request);
    }
}

#[test]
fn a_listen_host_request_is_unchanged() {
    // Only a dedicated server adds the byte: a listen host's request is the one older masters
    // already decode.
    let listen = encode_request(&create_room(false)).expect("encodes");
    let dedicated = encode_request(&create_room(true)).expect("encodes");
    assert_eq!(dedicated.len(), listen.len() + 1);
}
