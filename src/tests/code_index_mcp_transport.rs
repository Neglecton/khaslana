use super::*;
use std::io::{BufReader, Cursor};

fn server(data: &tempfile::TempDir) -> McpServer {
    McpServer::for_multi_test(data.path().into())
}

#[test]
fn mixed_framing_preserves_ids_utf8_lengths_and_notification_silence() {
    let data = tempfile::tempdir().unwrap();
    let request = r#"{"jsonrpc":"2.0","id":"中文","method":"ping"}"#;
    let input = format!("{{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}}\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{request}{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}}\n", request.len());
    let mut output = Vec::new();
    serve(&server(&data), &mut BufReader::with_capacity(3, Cursor::new(input)), &mut output).unwrap();
    let mut reader = Cursor::new(output);
    let one = read_frame(&mut reader).unwrap().unwrap();
    assert!(one.content_length);
    assert_eq!(serde_json::from_slice::<Value>(&one.bytes).unwrap()["id"], "中文");
    let two = read_frame(&mut reader).unwrap().unwrap();
    assert!(!two.content_length);
    assert_eq!(serde_json::from_slice::<Value>(&two.bytes).unwrap()["id"], 2);
    assert!(read_frame(&mut reader).unwrap().is_none());
}

#[test]
fn oversized_line_is_rejected_without_consuming_the_unbounded_remainder() {
    let input = vec![b'x'; MAX_FRAME_BYTES + 200];
    let mut reader = Cursor::new(input);
    assert!(read_frame(&mut reader).is_err());
    assert_eq!(reader.position(), MAX_FRAME_BYTES as u64 + 1);
}

#[test]
fn oversized_or_invalid_headers_never_dispatch_their_body_as_requests() {
    let data = tempfile::tempdir().unwrap();
    for header in [format!("Content-Length: {}\r\n\r\n", MAX_FRAME_BYTES + 1),
        "Content-Length: invalid\r\n\r\n".into(),
        "Content-Length: 1\r\nContent-Length: 2\r\n\r\n".into(),
        format!("Content-Length: 1\r\nExtra: {}\r\n\r\n", "x".repeat(MAX_HEADER_BYTES))] {
        let input = format!("{header}{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}}\n");
        let mut output = Vec::new();
        assert!(serve(&server(&data), &mut Cursor::new(input), &mut output).is_err());
        assert!(output.is_empty());
    }
}

#[test]
fn invalid_utf8_is_a_parse_error_and_the_next_frame_still_works() {
    let data = tempfile::tempdir().unwrap();
    let mut input = b"Content-Length: 1\r\n\r\n".to_vec();
    input.push(0xff);
    input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}\n");
    let mut output = Vec::new();
    serve(&server(&data), &mut Cursor::new(input), &mut output).unwrap();
    let mut reader = Cursor::new(output);
    let error: Value = serde_json::from_slice(&read_frame(&mut reader).unwrap().unwrap().bytes).unwrap();
    assert_eq!(error["error"]["code"], -32700);
    assert!(error["id"].is_null());
    let reply: Value = serde_json::from_slice(&read_frame(&mut reader).unwrap().unwrap().bytes).unwrap();
    assert_eq!(reply["id"], 3);
}

#[test]
fn truncated_frame_is_not_executed() {
    let data = tempfile::tempdir().unwrap();
    let mut output = Vec::new();
    let error = serve(&server(&data), &mut Cursor::new(b"Content-Length: 100\r\n\r\n{}"), &mut output).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    assert!(output.is_empty());
}

#[test]
fn broken_output_stops_reading_more_requests() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> { Err(io::ErrorKind::BrokenPipe.into()) }
        fn flush(&mut self) -> io::Result<()> { Ok(()) }
    }
    let data = tempfile::tempdir().unwrap();
    let request = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n";
    let mut reader = Cursor::new([request.as_slice(), request.as_slice()].concat());
    assert_eq!(serve(&server(&data), &mut reader, &mut Broken).unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    assert_eq!(reader.position(), request.len() as u64);
}
