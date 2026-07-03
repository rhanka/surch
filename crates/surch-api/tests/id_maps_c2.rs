//! Lot C `C2` : couverture dediee au refactor des id maps (`documents`/
//! `document_ids`/`reverse_document_ids` -> `DenseIdMaps` + overlays dirty
//! dans `crates/surch-api/src/state.rs`).
//!
//! Ces tests exercent la resolution uid<->doc_id (`_mget`, `_count`,
//! `_search`) A LA FOIS avant le premier `_refresh` (overlay mutable
//! `forward_dirty`/`reverse_dirty`/`documents_dirty`) ET apres (snapshot
//! dense `DenseIdMaps`), ainsi que les cas delete/update/reinsert qui
//! exercent `deleted_since_dense` et la garantie "un doc_id retire n'est
//! jamais reutilise".

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use surch_api::app_router;
use tower::ServiceExt;

async fn response_json(response: axum::response::Response<Body>) -> serde_json::Value {
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    serde_json::from_slice(&body).expect("response body should be json")
}

async fn index_doc(router: &Router, index: &str, id: &str, source: &str) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/{index}/_doc/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(source.to_owned()))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::CREATED);
}

async fn bulk(router: &Router, index: &str, ndjson: String) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/{index}/_bulk"))
                .header("content-type", "application/x-ndjson")
                .body(Body::from(ndjson))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::OK);
}

async fn refresh(router: &Router, index: &str) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/{index}/_refresh"))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::OK);
}

async fn mget(router: &Router, index: &str, ids: &[&str]) -> serde_json::Value {
    let ids_json = serde_json::to_string(ids).expect("ids should serialise");
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/{index}/_mget"))
                .header("content-type", "application/json")
                .body(Body::from(format!("{{\"ids\":{ids_json}}}")))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

async fn count(router: &Router, index: &str) -> u64 {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/{index}/_count"))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await["count"]
        .as_u64()
        .expect("count should be a number")
}

/// Lot C `C2` livrable #7 : le round-trip uid<->doc_id doit etre correct
/// A LA FOIS depuis l'overlay mutable (avant `_refresh`) ET depuis le
/// snapshot dense (apres `_refresh`) — pour des `_id` volontairement HORS
/// ordre lexicographique, pour ne pas masquer un bug d'ordonnancement
/// derriere un corpus deja trie.
#[tokio::test]
async fn id_maps_uid_roundtrip_before_and_after_refresh() {
    let router = app_router();
    index_doc(&router, "catalog", "zeta", r#"{"title":"z"}"#).await;
    index_doc(&router, "catalog", "alpha", r#"{"title":"a"}"#).await;
    index_doc(&router, "catalog", "mid", r#"{"title":"m"}"#).await;

    // Avant `_refresh` : resolution via les overlays dirty
    // (`forward_dirty`/`reverse_dirty`/`documents_dirty`).
    let before = mget(&router, "catalog", &["zeta", "alpha", "mid", "missing"]).await;
    let docs = before["docs"].as_array().expect("docs array");
    assert_eq!(docs.len(), 4);
    assert_eq!(docs[0]["found"], true);
    assert_eq!(docs[0]["_source"], serde_json::json!({"title": "z"}));
    assert_eq!(docs[1]["found"], true);
    assert_eq!(docs[1]["_source"], serde_json::json!({"title": "a"}));
    assert_eq!(docs[2]["found"], true);
    assert_eq!(docs[2]["_source"], serde_json::json!({"title": "m"}));
    assert_eq!(docs[3]["found"], false);
    assert_eq!(count(&router, "catalog").await, 3);

    refresh(&router, "catalog").await;

    // Apres `_refresh` : densify a tourne, resolution via le snapshot
    // dense (FST forward + buffers reverse/documents empaquetes).
    let after = mget(&router, "catalog", &["zeta", "alpha", "mid", "missing"]).await;
    let docs = after["docs"].as_array().expect("docs array");
    assert_eq!(docs[0]["found"], true);
    assert_eq!(docs[0]["_source"], serde_json::json!({"title": "z"}));
    assert_eq!(docs[1]["found"], true);
    assert_eq!(docs[1]["_source"], serde_json::json!({"title": "a"}));
    assert_eq!(docs[2]["found"], true);
    assert_eq!(docs[2]["_source"], serde_json::json!({"title": "m"}));
    assert_eq!(docs[3]["found"], false);
    assert_eq!(count(&router, "catalog").await, 3);
}

/// Un update conserve le MEME `doc_id` (verifie indirectement : le
/// `_count` ne bouge jamais, et le contenu observe est toujours le
/// DERNIER ecrit) — que l'update arrive avant ou apres que le document
/// ait ete densifie.
#[tokio::test]
async fn id_maps_update_keeps_identity_across_refresh() {
    let router = app_router();
    index_doc(&router, "catalog", "x", r#"{"title":"first"}"#).await;
    refresh(&router, "catalog").await; // "x" devient un doc_id densifie.

    // Update d'un doc DEJA densifie, avant le refresh suivant : va dans
    // `documents_dirty` (le doc_id ne change pas, `forward_dirty` reste
    // vide pour "x").
    index_doc(&router, "catalog", "x", r#"{"title":"second"}"#).await;
    assert_eq!(count(&router, "catalog").await, 1);
    let before = mget(&router, "catalog", &["x"]).await;
    assert_eq!(
        before["docs"][0]["_source"],
        serde_json::json!({"title": "second"}),
        "dirty override must win over the stale densified blob"
    );

    refresh(&router, "catalog").await;

    assert_eq!(count(&router, "catalog").await, 1);
    let after = mget(&router, "catalog", &["x"]).await;
    assert_eq!(
        after["docs"][0]["_source"],
        serde_json::json!({"title": "second"}),
        "the update must survive densify, not be shadowed by the pre-update dense snapshot"
    );
}

/// Delete via `_bulk` (le seul chemin HTTP qui exerce
/// `delete_document_deferred`) doit retirer le document des trois vues
/// publiques (`_mget`, `_count`, `_search match_all`) — avant ET apres
/// que la suppression ait ete "densifiee" par `_refresh`.
#[tokio::test]
async fn id_maps_delete_removes_document_and_updates_count() {
    let router = app_router();
    index_doc(&router, "catalog", "keep", r#"{"title":"keep"}"#).await;
    index_doc(&router, "catalog", "drop", r#"{"title":"drop"}"#).await;
    refresh(&router, "catalog").await;
    assert_eq!(count(&router, "catalog").await, 2);

    bulk(
        &router,
        "catalog",
        "{\"delete\":{\"_id\":\"drop\"}}\n".to_owned(),
    )
    .await;

    // Avant le prochain `_refresh` : le tombstone `deleted_since_dense`
    // doit deja masquer "drop" (le doc_id supprime appartenait au
    // snapshot dense construit par le refresh precedent).
    assert_eq!(count(&router, "catalog").await, 1);
    let mid = mget(&router, "catalog", &["keep", "drop"]).await;
    assert_eq!(mid["docs"][0]["found"], true);
    assert_eq!(mid["docs"][1]["found"], false);

    refresh(&router, "catalog").await;

    assert_eq!(count(&router, "catalog").await, 1);
    let after = mget(&router, "catalog", &["keep", "drop"]).await;
    assert_eq!(after["docs"][0]["found"], true);
    assert_eq!(after["docs"][1]["found"], false);

    let search = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/catalog/_search")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"match_all":{}},"size":20}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    let body = response_json(search).await;
    assert_eq!(body["hits"]["total"]["value"].as_u64(), Some(1));
    assert_eq!(body["hits"]["hits"][0]["_id"], "keep");
}

/// Un `_id` supprime PUIS reinsere (avant le `_refresh` suivant) doit
/// obtenir un nouveau contenu visible immediatement (overlay dirty) et
/// apres densify — jamais l'ancien contenu, jamais "not found" (verifie
/// la garantie "un doc_id retire n'est jamais reutilise, et un uid
/// reinsere obtient toujours un doc_id neuf").
#[tokio::test]
async fn id_maps_delete_then_reinsert_same_id_sees_fresh_content() {
    let router = app_router();
    index_doc(&router, "catalog", "r", r#"{"title":"original"}"#).await;
    refresh(&router, "catalog").await; // "r" densifie avec un premier doc_id.

    bulk(
        &router,
        "catalog",
        "{\"delete\":{\"_id\":\"r\"}}\n\
         {\"index\":{\"_id\":\"r\"}}\n\
         {\"title\":\"reborn\"}\n"
            .to_owned(),
    )
    .await;

    // Avant refresh : l'overlay dirty doit deja montrer le contenu neuf,
    // pas l'ancien (tombstone) ni 404.
    let before = mget(&router, "catalog", &["r"]).await;
    assert_eq!(before["docs"][0]["found"], true);
    assert_eq!(
        before["docs"][0]["_source"],
        serde_json::json!({"title": "reborn"})
    );
    assert_eq!(count(&router, "catalog").await, 1);

    refresh(&router, "catalog").await;

    let after = mget(&router, "catalog", &["r"]).await;
    assert_eq!(after["docs"][0]["found"], true);
    assert_eq!(
        after["docs"][0]["_source"],
        serde_json::json!({"title": "reborn"}),
        "densify must carry the NEW doc_id's content, not resurrect the tombstoned one"
    );
    assert_eq!(count(&router, "catalog").await, 1);
}

/// Insert puis delete du MEME `_id` dans la meme fenetre bulk, jamais
/// densifie entre les deux : `forward_dirty`/`reverse_dirty`/
/// `documents_dirty` doivent etre retractes proprement (pas de trou
/// fantome dans `deleted_since_dense`, qui ne doit tombstoner que des
/// doc_id APPARTENANT au snapshot dense precedent).
#[tokio::test]
async fn id_maps_insert_then_delete_before_first_refresh_never_appears() {
    let router = app_router();
    bulk(
        &router,
        "catalog",
        "{\"index\":{\"_id\":\"ephemeral\"}}\n\
         {\"title\":\"never persisted\"}\n\
         {\"delete\":{\"_id\":\"ephemeral\"}}\n"
            .to_owned(),
    )
    .await;

    assert_eq!(count(&router, "catalog").await, 0);
    let before = mget(&router, "catalog", &["ephemeral"]).await;
    assert_eq!(before["docs"][0]["found"], false);

    refresh(&router, "catalog").await;

    assert_eq!(count(&router, "catalog").await, 0);
    let after = mget(&router, "catalog", &["ephemeral"]).await;
    assert_eq!(after["docs"][0]["found"], false);
}
