//! MVP L2b: golden BYTE tests of the response families deces-backend reads,
//! against the real `deces_index.yml` mapping fixture.
//!
//! Every expected body below is `es-source-derived`: written from the
//! Elasticsearch 8.6.1 serialisation code (tag `v8.6.1`, paths under
//! `server/src/main/java/org/elasticsearch/`), not from an ES run. The
//! source location is cited next to each expectation. Only wall-clock
//! values are masked before the comparison: `took` (every occurrence) and
//! the opaque `_scroll_id`.
//!
//! Byte order rules exercised (see `docs/wp-d-matchid/l2b-response-shapes.md`):
//! - search: `_scroll_id`, `took`, `timed_out`, `_shards{total,successful,
//!   skipped,failed}`, `hits{total{value,relation},max_score,hits}`,
//!   `aggregations` (action/search/SearchResponse.java:274-301,
//!   rest/action/RestActions.java:79-103, search/SearchHits.java:173-198);
//! - hit: `_index`, `_id`, `_score` (always, `null` when untracked),
//!   `_source` (document key order), `sort` last (search/SearchHit.java:
//!   636-723);
//! - msearch: `took`, `responses[]`, each item + `status`
//!   (action/search/MultiSearchResponse.java:153-171);
//! - count: `count`, `_shards` (rest/action/search/RestCountAction.java:88-106);
//! - aggregations in request order; terms `doc_count_error_upper_bound`,
//!   `sum_other_doc_count`, `buckets`; date_histogram bucket
//!   `key_as_string`, `key`, `doc_count`; composite `after_key` before
//!   `buckets`; cardinality `value`.

use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use serde_json::Value;
use surch_api::{app_router, app_router_with_state, state::AppState, AppRouterState};
use tower::ServiceExt;

const MAPPING_REAL: &str = include_str!("../../../tests/matchid_compat/deces/mapping_real.json");

const D1: &str = r#"{"UID":"u1","NOM":"DUPONT","PRENOM":"JEAN","SEXE":"M","SOURCE":"2020-m01","DATE_DECES":"20200115","DATE_DECES_NORM":"20200115","AGE_DECES":80,"DEPARTEMENT_DECES":"2A"}"#;
const D2: &str = r#"{"UID":"u2","NOM":"MARTIN","PRENOM":"PAUL","SEXE":"M","SOURCE":"2020-m03","DATE_DECES":"20200310","DATE_DECES_NORM":"20200310","AGE_DECES":100,"DEPARTEMENT_DECES":"75"}"#;
const D3: &str = r#"{"UID":"u3","NOM":"DURAND","PRENOM":"LUCIE","SEXE":"F","SOURCE":"2020-m03","DATE_DECES":"20200302","DATE_DECES_NORM":"20200302","AGE_DECES":65,"DEPARTEMENT_DECES":"2A"}"#;

const SHARDS: &str = r#""_shards":{"total":1,"successful":1,"skipped":0,"failed":0}"#;

struct Reply {
    status: StatusCode,
    content_type: String,
    body: String,
}

async fn send(
    router: &Router,
    method: Method,
    uri: &str,
    body: &str,
    accept: Option<&str>,
) -> Reply {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(accept) = accept {
        request = request.header(header::ACCEPT, accept);
    }
    let response = router
        .clone()
        .oneshot(
            request
                .body(Body::from(body.to_owned()))
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    Reply {
        status,
        content_type,
        body: String::from_utf8(bytes.to_vec()).expect("utf8 body"),
    }
}

/// Replace the value of every `"<key>":<value>` occurrence with `0`
/// (number) or `"_"` (string).
fn mask(text: &str, key: &str) -> String {
    let needle = format!("\"{key}\":");
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(position) = rest.find(&needle) {
        out.push_str(&rest[..position + needle.len()]);
        rest = &rest[position + needle.len()..];
        if let Some(stripped) = rest.strip_prefix('"') {
            let end = stripped.find('"').expect("closing quote");
            out.push_str("\"_\"");
            rest = &stripped[end + 1..];
        } else {
            let end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            out.push('0');
            rest = &rest[end..];
        }
    }
    out.push_str(rest);
    out
}

fn masked(text: &str) -> String {
    mask(&mask(text, "took"), "_scroll_id")
}

async fn deces_router() -> Router {
    let router = app_router();
    load_deces(&router).await;
    router
}

async fn load_deces(router: &Router) {
    let created = send(router, Method::PUT, "/deces", MAPPING_REAL, None).await;
    assert_eq!(created.status, StatusCode::OK, "{}", created.body);
    let mut ndjson = String::new();
    for (id, doc) in [("d1", D1), ("d2", D2), ("d3", D3)] {
        ndjson.push_str(&format!(
            "{{\"index\":{{\"_index\":\"deces\",\"_id\":\"{id}\"}}}}\n"
        ));
        ndjson.push_str(doc);
        ndjson.push('\n');
    }
    let bulk = send(router, Method::POST, "/deces/_bulk", &ndjson, None).await;
    assert_eq!(bulk.status, StatusCode::OK, "{}", bulk.body);
    let refresh = send(router, Method::POST, "/deces/_refresh", "", None).await;
    assert_eq!(refresh.status, StatusCode::OK, "{}", refresh.body);
}

async fn search(router: &Router, body: &str) -> String {
    let reply = send(router, Method::POST, "/deces/_search", body, None).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    masked(&reply.body)
}

// es-source-derived: SearchResponse.java:274-301 + SearchHits.java:173-198
// + SearchHit.java:636-723. A field sort without a `_score` clause and
// without `track_scores` leaves `_score` and `max_score` `NaN` -> `null`
// (SearchPhaseController.java:384-392, 421-428; TopDocsCollectorContext.
// java:278-292); keyword sort values are the doc-values terms, i.e. after
// the `norm` normalizer (`2020-m03` -> `2020 m03`); no `_source` filter
// writes the stored source verbatim (FetchSourcePhase.java: source as-is).
#[tokio::test]
async fn version_shape_sorted_top_is_es_byte_identical_on_both_paths() {
    let router = deces_router().await;
    let expected = format!(
        concat!(
            r#"{{"took":0,"timed_out":false,{shards},"hits":{{"total":{{"value":3,"relation":"eq"}},"#,
            r#""max_score":null,"hits":[{{"_index":"deces","_id":"d2","_score":null,"_source":{d2},"#,
            r#""sort":["2020 m03","20200310"]}}]}}}}"#
        ),
        shards = SHARDS,
        d2 = D2
    );
    // deces-backend `status.controller.ts` /version body: served by the
    // term-dictionary walk.
    let fast = search(
        &router,
        r#"{"sort":[{"SOURCE":"desc"},{"DATE_DECES.raw":"desc"}],"size":1}"#,
    )
    .await;
    assert_eq!(fast, expected);

    // Same request through the general path (`min_score` makes the walk
    // decline; every hit scores the `match_all` constant 1.0 >= 0).
    let general = search(
        &router,
        r#"{"sort":[{"SOURCE":"desc"},{"DATE_DECES.raw":"desc"}],"size":1,"min_score":0}"#,
    )
    .await;
    assert_eq!(general, expected);

    // Whole order: SOURCE desc, then DATE_DECES.raw desc.
    let all_fast = search(
        &router,
        r#"{"sort":[{"SOURCE":"desc"},{"DATE_DECES.raw":"desc"}],"size":3,"_source":["UID"]}"#,
    )
    .await;
    let all_general = search(
        &router,
        r#"{"sort":[{"SOURCE":"desc"},{"DATE_DECES.raw":"desc"}],"size":3,"_source":["UID"],"min_score":0}"#,
    )
    .await;
    assert_eq!(all_fast, all_general);
    assert!(
        all_fast.contains(concat!(
            r#""hits":[{"_index":"deces","_id":"d2","_score":null,"_source":{"UID":"u2"},"sort":["2020 m03","20200310"]},"#,
            r#"{"_index":"deces","_id":"d3","_score":null,"_source":{"UID":"u3"},"sort":["2020 m03","20200302"]},"#,
            r#"{"_index":"deces","_id":"d1","_score":null,"_source":{"UID":"u1"},"sort":["2020 m01","20200115"]}]"#
        )),
        "{all_fast}"
    );
}

// es-source-derived: SearchSourceBuilder `_source` includes are applied by
// `SourceFilter.filterBytes` (search/lookup/SourceFilter.java:63-107): a
// Jackson filtering copy of the stored bytes, so the kept keys come out in
// DOCUMENT order (UID, NOM, AGE_DECES), neither in include order nor
// sorted. `match_all` scores its boost, 1.0 (Lucene `MatchAllDocsQuery`).
#[tokio::test]
async fn match_all_hit_with_source_filter_keeps_document_key_order() {
    let router = deces_router().await;
    let body = search(
        &router,
        r#"{"query":{"bool":{"must":[{"match_all":{}}]}},"_source":["AGE_DECES","UID","NOM"],"size":1,"track_total_hits":true}"#,
    )
    .await;
    let expected = format!(
        concat!(
            r#"{{"took":0,"timed_out":false,{shards},"hits":{{"total":{{"value":3,"relation":"eq"}},"#,
            r#""max_score":1.0,"hits":[{{"_index":"deces","_id":"d1","_score":1.0,"#,
            r#""_source":{{"UID":"u1","NOM":"DUPONT","AGE_DECES":80}}}}]}}}}"#
        ),
        shards = SHARDS
    );
    assert_eq!(body, expected);
}

// es-source-derived: a request without `query` is `match_all` (score 1.0);
// the unfiltered `_source` is the stored document (compact input here).
#[tokio::test]
async fn no_query_scores_one_and_returns_the_stored_source() {
    let router = deces_router().await;
    let body = search(&router, r#"{"size":1}"#).await;
    let expected = format!(
        concat!(
            r#"{{"took":0,"timed_out":false,{shards},"hits":{{"total":{{"value":3,"relation":"eq"}},"#,
            r#""max_score":1.0,"hits":[{{"_index":"deces","_id":"d1","_score":1.0,"_source":{d1}}}]}}}}"#
        ),
        shards = SHARDS,
        d1 = D1
    );
    assert_eq!(body, expected);
}

// es-source-derived: InternalDateHistogram.java:119-134 (bucket
// `key_as_string`, `key`, `doc_count`), :535-550 (`buckets` array),
// `min_doc_count` default 0 -> the empty February 2020 bucket between
// the first and the last one (`InternalDateHistogram.addEmptyBuckets`);
// `size: 0` -> `max_score: null` (TopDocsCollectorContext.java:447-455).
#[tokio::test]
async fn date_histogram_agg_is_es_byte_identical() {
    let router = deces_router().await;
    let body = search(
        &router,
        r#"{"size":0,"track_total_hits":true,"query":{"bool":{"must":[{"match_all":{}}]}},"aggs":{"deathDate":{"date_histogram":{"field":"DATE_DECES_NORM","calendar_interval":"month","format":"yyyyMMdd"}}}}"#,
    )
    .await;
    let expected = format!(
        concat!(
            r#"{{"took":0,"timed_out":false,{shards},"hits":{{"total":{{"value":3,"relation":"eq"}},"max_score":null,"hits":[]}},"#,
            r#""aggregations":{{"deathDate":{{"buckets":["#,
            r#"{{"key_as_string":"20200101","key":1577836800000,"doc_count":1}},"#,
            r#"{{"key_as_string":"20200201","key":1580515200000,"doc_count":0}},"#,
            r#"{{"key_as_string":"20200301","key":1583020800000,"doc_count":2}}]}}}}}}"#
        ),
        shards = SHARDS
    );
    assert_eq!(body, expected);

    // Without a request `format` the field format (`yyyyMMdd`) prints it.
    let body = search(
        &router,
        r#"{"size":0,"aggs":{"deathDate":{"date_histogram":{"field":"DATE_DECES_NORM","calendar_interval":"month"}}}}"#,
    )
    .await;
    assert!(
        body.contains(r#"{"key_as_string":"20200201","key":1580515200000,"doc_count":0}"#),
        "{body}"
    );
}

// es-source-derived: aggregations in request order (Aggregations.java:
// 117-122); terms `doc_count_error_upper_bound`, `sum_other_doc_count`,
// `buckets` (AbstractInternalTerms.java:365-380), bucket `key`,
// `doc_count` (InternalTerms.java:125-135); keyword keys are the
// normalised terms (`2A` -> `2a`); cardinality `value`
// (InternalCardinality.java:96-100).
#[tokio::test]
async fn terms_and_cardinality_aggs_are_es_byte_identical() {
    let router = deces_router().await;
    let body = search(
        &router,
        r#"{"size":0,"aggs":{"z_dept":{"terms":{"field":"DEPARTEMENT_DECES","size":1}},"a_count":{"cardinality":{"field":"DEPARTEMENT_DECES"}}}}"#,
    )
    .await;
    let expected = format!(
        concat!(
            r#"{{"took":0,"timed_out":false,{shards},"hits":{{"total":{{"value":3,"relation":"eq"}},"max_score":null,"hits":[]}},"#,
            r#""aggregations":{{"z_dept":{{"doc_count_error_upper_bound":0,"sum_other_doc_count":1,"#,
            r#""buckets":[{{"key":"2a","doc_count":2}}]}},"a_count":{{"value":2}}}}}}"#
        ),
        shards = SHARDS
    );
    assert_eq!(body, expected);
}

// es-source-derived: CompositeAggregation.java:41-51 (`after_key` before
// `buckets`), :31-39 (bucket `key` object in source order, `doc_count`);
// numeric terms sources compare as numbers (65 < 80 < 100), not as text.
#[tokio::test]
async fn composite_agg_is_es_byte_identical() {
    let router = deces_router().await;
    let body = search(
        &router,
        r#"{"size":0,"aggs":{"bucketResults":{"composite":{"size":1000,"sources":[{"sex":{"terms":{"field":"SEXE"}}},{"deathAge":{"terms":{"field":"AGE_DECES"}}}]}},"sex_count":{"cardinality":{"field":"SEXE"}}}}"#,
    )
    .await;
    let expected = format!(
        concat!(
            r#"{{"took":0,"timed_out":false,{shards},"hits":{{"total":{{"value":3,"relation":"eq"}},"max_score":null,"hits":[]}},"#,
            r#""aggregations":{{"bucketResults":{{"after_key":{{"sex":"M","deathAge":100}},"buckets":["#,
            r#"{{"key":{{"sex":"F","deathAge":65}},"doc_count":1}},"#,
            r#"{{"key":{{"sex":"M","deathAge":80}},"doc_count":1}},"#,
            r#"{{"key":{{"sex":"M","deathAge":100}},"doc_count":1}}]}},"sex_count":{{"value":2}}}}}}"#
        ),
        shards = SHARDS
    );
    assert_eq!(body, expected);

    // A composite date_histogram source with `format` prints the bucket
    // start with it (InternalComposite.formatObject).
    let body = search(
        &router,
        r#"{"size":0,"aggs":{"b":{"composite":{"sources":[{"deathDate":{"date_histogram":{"field":"DATE_DECES_NORM","calendar_interval":"month","format":"yyyyMMdd"}}}]}}}}"#,
    )
    .await;
    assert!(
        body.contains(concat!(
            r#""b":{"after_key":{"deathDate":"20200301"},"buckets":["#,
            r#"{"key":{"deathDate":"20200101"},"doc_count":1},{"key":{"deathDate":"20200301"},"doc_count":2}]}"#
        )),
        "{body}"
    );
}

// es-source-derived: MultiSearchResponse.java:153-171 — `took`, then
// `responses`, each the search body followed by `status`; a disabled
// `track_total_hits` omits `hits.total` (SearchHits.java:179-184).
#[tokio::test]
async fn msearch_envelope_is_es_byte_identical() {
    let router = deces_router().await;
    let ndjson = concat!(
        "{\"index\":\"deces\"}\n",
        r#"{"query":{"match_all":{}},"_source":["UID"],"size":1,"track_total_hits":false}"#,
        "\n"
    );
    let reply = send(&router, Method::POST, "/_msearch", ndjson, None).await;
    assert_eq!(reply.status, StatusCode::OK);
    let expected = format!(
        concat!(
            r#"{{"took":0,"responses":[{{"took":0,"timed_out":false,{shards},"#,
            r#""hits":{{"max_score":1.0,"hits":[{{"_index":"deces","_id":"d1","_score":1.0,"_source":{{"UID":"u1"}}}}]}},"#,
            r#""status":200}}]}}"#
        ),
        shards = SHARDS
    );
    assert_eq!(masked(&reply.body), expected);
}

// es-source-derived: RestCountAction.java:88-106 — `count`, then
// `_shards` with `skipped: 0`.
#[tokio::test]
async fn count_is_es_byte_identical() {
    let router = deces_router().await;
    let reply = send(&router, Method::GET, "/deces/_count", "", None).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body, format!(r#"{{"count":3,{SHARDS}}}"#));
}

// es-source-derived: SearchResponse.java:275-277 — `_scroll_id` is the
// FIRST key of the initial page and of every scroll page, the empty page
// that ends the stream included; a scroll page's `max_score` is the best
// score of that page (`null` when empty).
#[tokio::test]
async fn scroll_pages_put_scroll_id_first() {
    let router = deces_router().await;
    let first = send(
        &router,
        Method::POST,
        "/deces/_search?scroll=1m",
        r#"{"size":2,"_source":["UID"],"query":{"match_all":{}}}"#,
        None,
    )
    .await;
    assert_eq!(first.status, StatusCode::OK);
    let expected_first = format!(
        concat!(
            r#"{{"_scroll_id":"_","took":0,"timed_out":false,{shards},"hits":{{"total":{{"value":3,"relation":"eq"}},"#,
            r#""max_score":1.0,"hits":[{{"_index":"deces","_id":"d1","_score":1.0,"_source":{{"UID":"u1"}}}},"#,
            r#"{{"_index":"deces","_id":"d2","_score":1.0,"_source":{{"UID":"u2"}}}}]}}}}"#
        ),
        shards = SHARDS
    );
    assert_eq!(masked(&first.body), expected_first);

    let scroll_id = serde_json::from_str::<Value>(&first.body).unwrap()["_scroll_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let body = format!(r#"{{"scroll":"1m","scroll_id":"{scroll_id}"}}"#);
    let second = send(&router, Method::POST, "/_search/scroll", &body, None).await;
    assert_eq!(second.status, StatusCode::OK);
    let expected_second = format!(
        concat!(
            r#"{{"_scroll_id":"_","took":0,"timed_out":false,{shards},"hits":{{"total":{{"value":3,"relation":"eq"}},"#,
            r#""max_score":1.0,"hits":[{{"_index":"deces","_id":"d3","_score":1.0,"_source":{{"UID":"u3"}}}}]}}}}"#
        ),
        shards = SHARDS
    );
    assert_eq!(masked(&second.body), expected_second);

    let scroll_id = serde_json::from_str::<Value>(&second.body).unwrap()["_scroll_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let body = format!(r#"{{"scroll":"1m","scroll_id":"{scroll_id}"}}"#);
    let third = send(&router, Method::POST, "/_search/scroll", &body, None).await;
    assert_eq!(third.status, StatusCode::OK);
    let expected_third = format!(
        concat!(
            r#"{{"_scroll_id":"_","took":0,"timed_out":false,{shards},"hits":{{"total":{{"value":3,"relation":"eq"}},"#,
            r#""max_score":null,"hits":[]}}}}"#
        ),
        shards = SHARDS
    );
    assert_eq!(masked(&third.body), expected_third);
}

fn is_strict_date_time(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 24
        && bytes.iter().enumerate().all(|(i, b)| match i {
            4 | 7 => *b == b'-',
            10 => *b == b'T',
            13 | 16 => *b == b':',
            19 => *b == b'.',
            23 => *b == b'Z',
            _ => b.is_ascii_digit(),
        })
}

// es-source-derived: RestIndicesAction.java:266-267, 678-680
// (`creation.date` millis, `creation.date.string` = `strict_date_time`
// in UTC), RestTable.java:59-75 (JSON rows: one object per index, keys =
// requested headers in order, string values).
#[tokio::test]
async fn cat_indices_creation_date_string_as_deces_backend_reads_it() {
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let router = deces_router().await;
    let after = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    // deces-backend `client.cat.indices({index, format:'json',
    // h:'creation.date.string'})`.
    let reply = send(
        &router,
        Method::GET,
        "/_cat/indices/deces?format=json&h=creation.date.string",
        "",
        Some("application/vnd.elasticsearch+json; compatible-with=8,text/plain"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    let rows: Value = serde_json::from_str(&reply.body).unwrap();
    let rows = rows.as_array().expect("array of rows");
    assert_eq!(rows.len(), 1);
    let row = rows[0].as_object().expect("row object");
    assert_eq!(row.len(), 1);
    let date = row["creation.date.string"].as_str().expect("string value");
    assert!(is_strict_date_time(date), "{date}");
    assert_eq!(
        reply.body,
        format!(r#"[{{"creation.date.string":"{date}"}}]"#)
    );

    // `creation.date` (by name and alias `cd`) is the same instant.
    let reply = send(
        &router,
        Method::GET,
        "/_cat/indices/deces?format=json&h=creation.date,cd",
        "",
        None,
    )
    .await;
    let rows: Value = serde_json::from_str(&reply.body).unwrap();
    let millis: i64 = rows[0]["creation.date"].as_str().unwrap().parse().unwrap();
    assert_eq!(rows[0]["cd"], rows[0]["creation.date"]);
    assert!(before <= millis && millis <= after, "{millis}");
    // Same date part as the string column (what the backend keeps).
    let day = millis.div_euclid(86_400_000);
    let string_day = {
        let y: i64 = date[0..4].parse().unwrap();
        let m: i64 = date[5..7].parse().unwrap();
        let d: i64 = date[8..10].parse().unwrap();
        let y2 = if m <= 2 { y - 1 } else { y };
        let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
        let yoe = y2 - era * 400;
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
        era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
    };
    assert_eq!(day, string_day);

    // Through the alias the backend searches.
    let alias = send(
        &router,
        Method::POST,
        "/_aliases",
        r#"{"actions":[{"add":{"index":"deces","alias":"deces-live"}}]}"#,
        None,
    )
    .await;
    assert_eq!(alias.status, StatusCode::OK, "{}", alias.body);
    let via_alias = send(
        &router,
        Method::GET,
        "/_cat/indices/deces-live?format=json&h=index,creation.date.string",
        "",
        None,
    )
    .await;
    assert_eq!(
        via_alias.body,
        format!(r#"[{{"index":"deces","creation.date.string":"{date}"}}]"#)
    );

    // A missing concrete index is a 404 (IndicesOptions.strictExpand).
    let missing = send(
        &router,
        Method::GET,
        "/_cat/indices/nope?format=json",
        "",
        None,
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert!(missing.body.contains("index_not_found_exception"));
}

// es-source-derived: RestTable.java:77-113, 300-330 (text table: cells
// padded to the column width, `text-align:right` columns left-padded, one
// space between cells, nothing after the last cell, `\n` per row, header
// row only with `v`); `deces-*` expands over index names; a wildcard
// matching nothing is an empty table.
#[tokio::test]
async fn cat_indices_text_table_for_the_restore_job_forms() {
    let router = app_router();
    for index in ["deces-202601", "deces-202602", "other"] {
        let created = send(&router, Method::PUT, &format!("/{index}"), "", None).await;
        assert_eq!(created.status, StatusCode::OK);
    }
    // elasticsearch-restore.job.yaml: `curl "$ES_URL/_cat/indices/$ES_INDEX-*?h=index"`.
    let reply = send(
        &router,
        Method::GET,
        "/_cat/indices/deces-*?h=index",
        "",
        Some("*/*"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert!(
        reply.content_type.starts_with("text/plain"),
        "{}",
        reply.content_type
    );
    assert_eq!(reply.body, "deces-202601\ndeces-202602\n");

    let reply = send(
        &router,
        Method::GET,
        "/_cat/indices/deces-*?h=index,docs.count&v=true&format=txt",
        "",
        None,
    )
    .await;
    assert_eq!(
        reply.body,
        "index        docs.count\ndeces-202601          0\ndeces-202602          0\n"
    );

    let reply = send(
        &router,
        Method::GET,
        "/_cat/indices/nothing-*?h=index",
        "",
        Some("*/*"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body, "");
}

/// A throwaway data directory under `TMPDIR` (never `/tmp` directly in CI
/// runs that set it).
struct TestDir(std::path::PathBuf);

impl TestDir {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir()
            .join("l2b-data")
            .join(format!("{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// MVP L2b + L6: the creation date is part of the persisted index settings,
// so `_cat/indices` reports the same instant after a restart.
#[tokio::test]
async fn creation_date_survives_a_restart() {
    let dir = TestDir::new("creation-date");
    let read = |router: Router| async move {
        let reply = send(
            &router,
            Method::GET,
            "/_cat/indices/deces?format=json&h=creation.date,creation.date.string",
            "",
            None,
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
        reply.body
    };
    let first = {
        let app = AppState::open_with_data_dir(&dir.0).expect("data dir opens");
        let router = app_router_with_state(AppRouterState {
            app: app.clone(),
            ..Default::default()
        });
        load_deces(&router).await;
        read(router).await
    };
    let app = AppState::open_with_data_dir(&dir.0).expect("data dir reopens");
    let router = app_router_with_state(AppRouterState {
        app: app.clone(),
        ..Default::default()
    });
    let second = read(router).await;
    assert_eq!(first, second);
    assert!(first.contains("\"creation.date\":\""), "{first}");
}
