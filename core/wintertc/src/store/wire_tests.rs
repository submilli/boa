use super::*;
use boa_engine::{Context, Source, js_string, value::TryIntoJs};

#[test]
fn storage_transport_preserves_cycles_utf16_and_buffer_aliases() {
    let mut source = Context::default();
    let value = source.eval(Source::from_bytes("let b=new ArrayBuffer(8);let a={text:'\\ud800',big:12345678901234567890n,map:new Map(),set:new Set(),buffer:b,view:new Uint16Array(b,2,2),data:new DataView(b),boxed:new Number(3),date:new Date(123),regex:/abc/gi,error:new TypeError('bad')};a.self=a;a.map.set(a,a.view);a.set.add(a);a.error.cause=a;a")).unwrap();
    let bytes = JsValueStore::for_storage(&value, &mut source)
        .unwrap()
        .to_storage_bytes()
        .unwrap();
    let store = JsValueStore::from_storage_bytes(&bytes).unwrap();
    let mut target = Context::default();
    let copy = store.try_into_js(&mut target).unwrap();
    target
        .register_global_property(
            js_string!("copy"),
            copy,
            boa_engine::property::Attribute::all(),
        )
        .unwrap();
    assert!(target.eval(Source::from_bytes("copy.self===copy && copy.text.charCodeAt(0)===0xd800 && copy.big===12345678901234567890n && copy.map.get(copy)===copy.view && copy.set.has(copy) && copy.view.buffer===copy.buffer && copy.data.buffer===copy.buffer && copy.boxed.valueOf()===3 && +copy.date===123 && copy.regex.source==='abc' && copy.regex.flags==='gi' && copy.error.cause===copy && copy.error instanceof TypeError")).unwrap().to_boolean());
}

#[test]
fn malformed_storage_transport_rejects_before_reconstruction() {
    let mut cx = Context::default();
    let value = cx.eval(Source::from_bytes("({value:1})")).unwrap();
    let bytes = JsValueStore::for_storage(&value, &mut cx)
        .unwrap()
        .to_storage_bytes()
        .unwrap();
    for cut in 0..bytes.len() {
        assert!(JsValueStore::from_storage_bytes(&bytes[..cut]).is_err());
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(JsValueStore::from_storage_bytes(&trailing).is_err());
    let mut badroot = bytes.clone();
    badroot[4..12].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(JsValueStore::from_storage_bytes(&badroot).is_err());
    let mut badcount = bytes.clone();
    badcount[12..20].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(JsValueStore::from_storage_bytes(&badcount).is_err());
    let mut unknown = bytes;
    unknown[20] = 255;
    assert!(JsValueStore::from_storage_bytes(&unknown).is_err());
    assert!(JsValueStore::from_storage_bytes(&vec![0; MAX_BYTES + 1]).is_err());
}

#[test]
fn decoded_edges_types_and_depth_are_validated() {
    for nodes in [
        vec![Node::Boxed(1)],
        vec![Node::Null, Node::Boxed(0)],
        vec![
            Node::Null,
            Node::DataView {
                buffer: 0,
                byte_length: Some(1),
                byte_offset: 0,
            },
        ],
    ] {
        assert!(validate::graph(&nodes, 0).is_err());
    }
    let mut nodes = vec![Node::Null];
    for id in 0..65 {
        nodes.push(Node::Object(vec![(StringStore(vec![97]), id)]));
    }
    assert!(validate::graph(&nodes, 65).is_err());
}

fn roundtrip_expression(expression: &str, assertion: &str) {
    let mut cx = Context::default();
    let value = cx.eval(Source::from_bytes(expression)).unwrap();
    let encoded = JsValueStore::for_storage(&value, &mut cx)
        .unwrap()
        .to_storage_bytes()
        .unwrap();
    let store = JsValueStore::from_storage_bytes(&encoded).unwrap();
    let copy = store.try_into_js(&mut cx).unwrap();
    cx.register_global_property(
        js_string!("copy"),
        copy,
        boa_engine::property::Attribute::all(),
    )
    .unwrap();
    assert!(cx.eval(Source::from_bytes(assertion)).unwrap().to_boolean());
}
#[test]
fn float16_offsets_and_shared_aliases_roundtrip() {
    roundtrip_expression(
        "(()=>{let b=new ArrayBuffer(8);let view=new Float16Array(b,2,2);view[0]=1.5;return {b,view}})()",
        "copy.view instanceof Float16Array && copy.view.buffer===copy.b && copy.view.byteOffset===2 && copy.view[0]===1.5",
    );
}
#[test]
fn heavily_shared_map_does_not_exhaust_a_node_sized_traversal_queue() {
    roundtrip_expression(
        "(()=>{let shared={x:1};return new Map(Array.from({length:32768},(_,i)=>[i,shared]))})()",
        "copy.size===32768 && copy.get(0)===copy.get(32767) && copy.get(1).x===1",
    );
}
#[test]
fn resizable_maximum_and_tracking_views_roundtrip() {
    roundtrip_expression(
        "(()=>{let b=new ArrayBuffer(8,{maxByteLength:16777217});return {b,view:new Uint8Array(b,2)}})()",
        "copy.b.resizable && copy.b.maxByteLength===16777217 && copy.view.buffer===copy.b && copy.view.length===6",
    );
}
#[test]
fn array_field_invariants_are_checked_before_reconstruction() {
    for (length, key) in [(0, "4294967294"), (1, "1"), (1, "length")] {
        let nodes = vec![
            Node::Array {
                length,
                fields: vec![(StringStore(key.encode_utf16().collect()), 1)],
            },
            Node::Null,
        ];
        assert!(validate::graph(&nodes, 0).is_err());
    }
    roundtrip_expression(
        "(()=>{let a=new Array(7);a[4]='value';a['01']='named';return a})()",
        "copy.length===7 && !(0 in copy) && copy[4]==='value' && copy['01']==='named'",
    );
}
#[test]
fn decoded_retained_size_accounts_for_actual_collection_capacities() {
    let mut cx = Context::default();
    let value=cx.eval(Source::from_bytes("[Object.fromEntries(Array.from({length:19},(_,i)=>['k'+i,i])),new Map(Array.from({length:37},(_,i)=>[i,i])),new Set(Array.from({length:41},(_,i)=>i))]")).unwrap();
    let encoded = JsValueStore::for_storage(&value, &mut cx)
        .unwrap()
        .to_storage_bytes()
        .unwrap();
    let store = JsValueStore::from_storage_bytes(&encoded).unwrap();
    let actual = store.graph.capacity() * size_of::<Node>()
        + store
            .graph
            .iter()
            .map(Node::retained_payload_bytes)
            .sum::<usize>();
    assert_eq!(store.retained_bytes(), actual);
}
#[test]
fn primitive_roots_and_numeric_special_values_roundtrip() {
    for (expression, assertion) in [
        ("null", "copy===null"),
        ("undefined", "copy===undefined"),
        ("true", "copy===true"),
        ("-0", "Object.is(copy,-0)"),
        ("NaN", "Number.isNaN(copy)"),
        ("Infinity", "copy===Infinity"),
        ("-12345678901234567890n", "copy===-12345678901234567890n"),
    ] {
        roundtrip_expression(expression, assertion);
    }
}
