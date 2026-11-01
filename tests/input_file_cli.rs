//! Input-file success mutation uses only a local HTTP fixture.
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::Command,
};

fn dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("galerith-input-{name}-{}", std::process::id()))
}

fn serve_one() -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/picture.jpg", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0_u8; 1024];
        assert!(socket.read(&mut request).unwrap() > 0);
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\nConnection: close\r\n\r\nJPEG",
            )
            .unwrap();
    });
    (url, server)
}

fn serve_retries(
    first: &'static [u8],
    second: &'static [u8],
) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/picture.jpg", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        for response in [first, second] {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            assert!(socket.read(&mut request).unwrap() > 0);
            socket.write_all(response).unwrap();
        }
    });
    (url, server)
}

#[test]
fn comments_successful_duplicate_lines_and_preserves_failed_entries() {
    let root = dir("comment-cli");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let input = root.join("urls.txt");
    let (url, server) = serve_one();
    fs::write(
        &input,
        format!("# keep this comment\n  {url}  \n{url}\nhttps://example.org/unsupported\n"),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(["-I", input.to_str().unwrap(), "-d"])
        .arg(&root)
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(!output.status.success(), "unsupported URL should fail");
    assert_eq!(
        fs::read_to_string(&input).unwrap(),
        format!("# keep this comment\n#   {url}  \n# {url}\nhttps://example.org/unsupported\n")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn deletes_successful_input_line_but_keeps_other_entries() {
    let root = dir("delete-cli");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let input = root.join("urls.txt");
    let (url, server) = serve_one();
    fs::write(&input, format!("{url}\n# https://example.org/later.jpg\n")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(["--input-file-delete", input.to_str().unwrap(), "-d"])
        .arg(&root)
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(&input).unwrap(),
        "# https://example.org/later.jpg\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn simulation_does_not_mutate_input_files() {
    let root = dir("simulate-cli");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let input = root.join("urls.txt");
    let content = "https://example.org/photo.jpg\n";
    fs::write(&input, content).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--input-file-comment",
            input.to_str().unwrap(),
            "--simulate",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(fs::read_to_string(&input).unwrap(), content);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn input_is_commented_after_a_download_succeeds_on_retry() {
    let root = dir("retry-success-cli");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let input = root.join("urls.txt");
    let (url, server) = serve_retries(
        b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\nConnection: close\r\n\r\nJPEG",
    );
    fs::write(&input, format!("{url}\n")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "--input-file-comment",
            input.to_str().unwrap(),
            "--retries",
            "1",
            "-d",
        ])
        .arg(&root)
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(&input).unwrap(), format!("# {url}\n"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn input_stays_active_after_all_download_retries_fail() {
    let root = dir("retry-failure-cli");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let input = root.join("urls.txt");
    let (url, server) = serve_retries(
        b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    let content = format!("{url}\n");
    fs::write(&input, &content).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(["-x", input.to_str().unwrap(), "--retries", "1", "-d"])
        .arg(&root)
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&input).unwrap(), content);
    fs::remove_dir_all(root).unwrap();
}
