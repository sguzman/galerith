//! Synthetic localhost checks for filename templates at the filesystem boundary.
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::Command,
};

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "galerith-filename-filesystem-{}",
        std::process::id()
    ))
}

#[test]
fn rendered_template_is_the_saved_path_and_existing_output_is_skipped() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0; 2048];
        assert!(socket.read(&mut request).unwrap() > 0);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nDATA")
            .unwrap();
        assert!(listener.set_nonblocking(true).is_ok());
        assert!(
            listener.accept().is_err(),
            "unexpected second media request"
        );
    });

    let directory = temp_dir();
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    let url = format!("http://{address}/source.webp");
    let args = [
        "-d".to_owned(),
        directory.to_string_lossy().into_owned(),
        "-f".to_owned(),
        "{{proof}}_{category}_{stem:Rsource/renamed/}.{extension}".to_owned(),
        url,
    ];
    let first = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(&args)
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    server.join().unwrap();

    let output = directory.join("{proof}_directlink_renamed.webp");
    assert_eq!(fs::read(&output).unwrap(), b"DATA");
    let second = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args(&args)
        .output()
        .unwrap();
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(String::from_utf8_lossy(&second.stdout).contains("skipped"));
    assert_eq!(fs::read(&output).unwrap(), b"DATA");
    assert!(!directory
        .join("{proof}_directlink_renamed.webp.part")
        .exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn template_cannot_escape_the_destination_directory() {
    let directory = std::env::temp_dir().join(format!(
        "galerith-filename-traversal-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_galerith"))
        .args([
            "-d",
            directory.to_str().unwrap(),
            "-f",
            "../escape-{filename}",
            "https://example.org/source.webp",
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(fs::read_dir(&directory).unwrap().next().is_none());
    assert!(!directory
        .parent()
        .unwrap()
        .join("escape-source.webp")
        .exists());
    fs::remove_dir_all(directory).unwrap();
}
