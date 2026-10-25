use serde_json::{json, Value};
use std::process::Stdio;
use std::time::Duration;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};
const BIN: &str = env!("CARGO_BIN_EXE_tezbar-amphetamine");
struct Peer {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    calls: Vec<Value>,
    sequence: u64,
}
impl Peer {
    async fn start() -> Self {
        let mut child = Command::new(BIN)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            input,
            output,
            calls: vec![],
            sequence: 1,
        }
    }
    async fn send(&mut self, value: Value) {
        let mut b = serde_json::to_vec(&value).unwrap();
        b.push(b'\n');
        self.input.as_mut().unwrap().write_all(&b).await.unwrap();
    }
    async fn frame(&mut self) -> Value {
        let mut line = String::new();
        let n = tokio::time::timeout(Duration::from_secs(5), self.output.read_line(&mut line))
            .await
            .expect("protocol timeout")
            .unwrap();
        assert!(n > 0, "unexpected EOF");
        serde_json::from_str(&line).unwrap()
    }
    async fn request(&mut self, _label: &str, method: &str, params: Value) -> Value {
        let wire_id = format!("h:00000000-0000-4000-8000-{:012x}", self.sequence);
        self.sequence += 1;
        let id = wire_id.as_str();
        self.send(json!({"type":"request","id":id,"method":method,"params":params}))
            .await;
        loop {
            let frame = self.frame().await;
            if frame["type"] == "response" && frame["id"] == id {
                assert!(frame.get("error").is_none(), "{frame}");
                return frame["result"].clone();
            }
            if frame["type"] == "request" {
                let method = frame["method"].as_str().unwrap();
                assert!(method.starts_with("host."), "non-generic RPC");
                let result = match method {
                    "host.selection.read" => json!("Café & tea"),
                    _ => json!(true),
                };
                self.calls.push(frame.clone());
                self.send(json!({"type":"response","id":frame["id"],"result":result}))
                    .await;
            } else {
                assert_eq!(frame["type"], "event");
            }
        }
    }
    async fn initialize(&mut self, dir: &std::path::Path) {
        let hello = self.frame().await;
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["id"], "com.tezbar.native.amphetamine");
        assert_eq!(hello["protocol"], 1);
        assert_eq!(hello["version"], env!("CARGO_PKG_VERSION"));
        self.request("h:init","initialize",json!({"protocol":1,"extensionId":"com.tezbar.native.amphetamine","platform":if cfg!(target_os="windows") {"x86_64-pc-windows-msvc"} else {"aarch64-apple-darwin"},"packageDir":env!("CARGO_MANIFEST_DIR"),"dataDir":dir.join("data"),"tempDir":dir.join("temp"),"grantedCapabilities":["background","selection","clipboard","shell","window","screen","apps"],"reason":"command","migrationPaths":{}})).await;
    }
}
#[tokio::test]
async fn real_protocol_info_hello_open_dispose_and_eof() {
    let output = Command::new(BIN)
        .arg("--protocol-info")
        .output()
        .await
        .unwrap();
    assert!(output.status.success());
    let info: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(info["protocol"], 1);
    assert_eq!(info["id"], "com.tezbar.native.amphetamine");
    let dir = tempfile::tempdir().unwrap();
    let mut peer = Peer::start().await;
    peer.initialize(dir.path()).await;
    let view=peer.request("h:open","session.open",json!({"sessionId":"portable-fixture","commandName":"amp-duration","preferences":{},"argumentValues":{}})).await;
    assert_eq!(view["mode"], "view");
    let disposed = peer
        .request(
            "h:dispose",
            "session.dispose",
            json!({"sessionId":"portable-fixture"}),
        )
        .await;
    assert!(disposed["ok"].as_bool().unwrap_or(true));
    peer.input.take();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), peer.child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert!(
        peer.calls.is_empty(),
        "view-only fixture must not mutate hardware or contact providers"
    );
}

#[tokio::test]
async fn real_shutdown_exits_with_input_still_open() {
    let dir = tempfile::tempdir().unwrap();
    let mut peer = Peer::start().await;
    peer.initialize(dir.path()).await;
    peer.request("h:shutdown", "shutdown", json!({})).await;
    assert!(peer.input.is_some());
    assert!(
        tokio::time::timeout(Duration::from_secs(5), peer.child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}
