use serde_json::json;

use super::{float_repr, json_dumps};

#[test]
fn float_repr_matches_python() {
    let cases = [
        (0.0, "0.0"),
        (-0.0, "-0.0"),
        (1000.0, "1000.0"),
        (1763456789.123456, "1763456789.123456"),
        (0.0001, "0.0001"),
        (0.00001, "1e-05"),
        (1e16, "1e+16"),
        (1.5e16, "1.5e+16"),
        (1234567890123456.0, "1234567890123456.0"),
        (0.1 + 0.2, "0.30000000000000004"),
        (-2.5e-7, "-2.5e-07"),
        (1e300, "1e+300"),
    ];
    for (x, expected) in cases {
        assert_eq!(float_repr(x), expected, "{x:?}");
    }
}

#[test]
fn json_dumps_matches_python() {
    let value = json!({"b": [1, {"z": null, "a": true}], "a": "é😀\"\\\n\u{7f}", "c": {}, "d": [], "e": 2.0});
    assert_eq!(
        json_dumps(&value, Some(2)),
        "{\n  \"a\": \"\\u00e9\\ud83d\\ude00\\\"\\\\\\n\\u007f\",\n  \"b\": [\n    1,\n    {\n      \"a\": true,\n      \"z\": null\n    }\n  ],\n  \"c\": {},\n  \"d\": [],\n  \"e\": 2.0\n}"
    );
    assert_eq!(
        json_dumps(&json!({"b": [1, 2], "a": -3}), None),
        "{\"a\":-3,\"b\":[1,2]}"
    );
}
