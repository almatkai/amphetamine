//! Imports only the explicit host-granted native store; never searches app data.
use serde_json::{json, Value};
use tezbar_extension_sdk::{read_store, update_store, CoreResult, ExtensionContext};
pub async fn initialize(ctx: &ExtensionContext, name: &str) -> CoreResult {
    let current = read_store(ctx, name).await?;
    if current["_nativeStoreImported"] == true {
        return Ok(current);
    }
    let legacy = if let Some(path) = ctx.migration_paths.get("nativeStore") {
        match tokio::fs::read(path).await {
            Ok(bytes) => {
                let value: Value = serde_json::from_slice(&bytes)
                    .map_err(|e| format!("Invalid native migration store: {e}"))?;
                if !value.is_object() {
                    return Err("Native migration store must be object".into());
                }
                value
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
            Err(e) => return Err(e.to_string()),
        }
    } else {
        json!({})
    };
    update_store(ctx, name, move |store| {
        let target = store.as_object_mut().ok_or("Store must be object")?;
        for (key, value) in legacy.as_object().unwrap() {
            target.entry(key.clone()).or_insert(value.clone());
        }
        target.insert("_nativeStoreImported".into(), json!(true));
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tezbar_extension_sdk::{ExtensionPaths, TestContext};
    #[tokio::test]
    async fn imports_only_granted_store_preserves_existing_and_does_not_modify_legacy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.json");
        let bytes = br#"{"legacy":7,"current":"old","future":{"keep":true}}"#;
        std::fs::write(&path, bytes).unwrap();
        let mut fixture = TestContext::new(
            ExtensionPaths {
                package_dir: dir.path().join("package"),
                data_dir: dir.path().join("private"),
                temp_dir: dir.path().join("tmp"),
            },
            &[],
        );
        fixture
            .context
            .migration_paths
            .insert("nativeStore".into(), path.clone());
        update_store(&fixture.context, "fixture", |s| {
            s["current"] = json!("new");
            Ok(())
        })
        .await
        .unwrap();
        let store = initialize(&fixture.context, "fixture").await.unwrap();
        assert_eq!(store["legacy"], 7);
        assert_eq!(store["current"], "new");
        assert_eq!(store["future"]["keep"], true);
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        assert_eq!(
            initialize(&fixture.context, "fixture").await.unwrap(),
            store
        );
        assert!(fixture.calls().is_empty());
    }
    #[tokio::test]
    async fn malformed_migration_is_rejected_without_overwriting_private_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, b"{broken").unwrap();
        let mut fixture = TestContext::new(
            ExtensionPaths {
                package_dir: dir.path().join("package"),
                data_dir: dir.path().join("private"),
                temp_dir: dir.path().join("tmp"),
            },
            &[],
        );
        fixture
            .context
            .migration_paths
            .insert("nativeStore".into(), path);
        assert!(initialize(&fixture.context, "fixture").await.is_err());
        assert_eq!(
            read_store(&fixture.context, "fixture").await.unwrap(),
            json!({})
        );
    }
}
