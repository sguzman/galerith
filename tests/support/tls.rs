use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub struct TestCertificates {
    pub ca: PathBuf,
    pub server: PathBuf,
    pub server_key: PathBuf,
}

pub fn create_ca_signed_server_certificate(directory: &Path) -> TestCertificates {
    fs::create_dir_all(directory).unwrap();
    let ca = directory.join("test-ca.pem");
    let ca_key = directory.join("test-ca-key.pem");
    let server = directory.join("server-cert.pem");
    let server_key = directory.join("server-key.pem");
    let request = directory.join("server.csr");
    let server_extensions = directory.join("server-ext.cnf");

    fs::write(&server_key, include_str!("../fixtures/tls/server-key.pem")).unwrap();

    let ca_result = Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            ca_key.to_str().unwrap(),
            "-out",
            ca.to_str().unwrap(),
            "-days",
            "3650",
            "-subj",
            "/CN=Galerith Test CA",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-addext",
            "keyUsage=critical,keyCertSign,cRLSign",
        ])
        .output()
        .expect("openssl is required for the local TLS fixture");
    assert!(
        ca_result.status.success(),
        "could not create test CA: {}",
        String::from_utf8_lossy(&ca_result.stderr)
    );

    let request_result = Command::new("openssl")
        .args([
            "req",
            "-new",
            "-key",
            server_key.to_str().unwrap(),
            "-out",
            request.to_str().unwrap(),
            "-subj",
            "/CN=127.0.0.1",
        ])
        .output()
        .expect("openssl is required for the local TLS fixture");
    assert!(
        request_result.status.success(),
        "could not create server certificate request: {}",
        String::from_utf8_lossy(&request_result.stderr)
    );

    fs::write(&server_extensions, "subjectAltName=IP:127.0.0.1\n").unwrap();
    let sign_result = Command::new("openssl")
        .args([
            "x509",
            "-req",
            "-in",
            request.to_str().unwrap(),
            "-CA",
            ca.to_str().unwrap(),
            "-CAkey",
            ca_key.to_str().unwrap(),
            "-CAcreateserial",
            "-extfile",
            server_extensions.to_str().unwrap(),
            "-out",
            server.to_str().unwrap(),
            "-days",
            "3650",
        ])
        .output()
        .expect("openssl is required for the local TLS fixture");
    assert!(
        sign_result.status.success(),
        "could not sign server certificate: {}",
        String::from_utf8_lossy(&sign_result.stderr)
    );

    TestCertificates {
        ca,
        server,
        server_key,
    }
}
