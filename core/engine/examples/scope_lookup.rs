//! Deterministic scope microbenchmarks and large-script startup measurements.
//!
//! Run with `cargo run --release -p boa_engine --example scope_lookup`.
#![allow(unused_crate_dependencies, clippy::print_stdout)] // A standalone measurement tool.
use boa_ast::scope::Scope;
use boa_engine::{Context, Script, Source};
use boa_string::JsString;
use std::{fmt::Write, hint::black_box, time::Instant};

fn main() {
    println!(
        "bindings,repeat,create_ns,hit_ns,miss_ns,outer_ns,access_ns,parse_ns,compile_ns,evaluate_ns"
    );
    for count in [8, 32, 128, 1024, 4096, 8192] {
        let names: Vec<_> = (0..count)
            .map(|i| JsString::from(format!("binding_{i:05}")))
            .collect();
        let missing = JsString::from("binding_missing");
        let source = script_source(count);
        for repeat in 0..6 {
            let start = Instant::now();
            let scope = Scope::new(Scope::new_global(), true);
            for name in &names {
                black_box(scope.create_mutable_binding(name.clone(), true));
            }
            let create = start.elapsed().as_nanos();
            let rounds = 16;
            let hit = measure(rounds * count, |i| {
                black_box(scope.get_identifier_reference(names[i % count].clone()));
            });
            let miss = measure(rounds * count, |_| {
                black_box(scope.get_identifier_reference(missing.clone()));
            });
            let child = Scope::new(scope.clone(), true);
            for i in 0..32 {
                black_box(child.create_mutable_binding(JsString::from(format!("child_{i}")), true));
            }
            let outer = measure(rounds * count, |i| {
                black_box(child.get_identifier_reference(names[i % count].clone()));
            });
            let access = measure(rounds * count, |i| {
                child.access_binding(&names[i % count], false);
            });
            let mut context = Context::default();
            let start = Instant::now();
            let script = Script::parse(Source::from_bytes(&source), None, &mut context).unwrap();
            let parse = start.elapsed().as_nanos();
            let start = Instant::now();
            black_box(script.codeblock(&mut context).unwrap());
            let compile = start.elapsed().as_nanos();
            let start = Instant::now();
            let result = script.evaluate(&mut context).unwrap();
            let evaluate = start.elapsed().as_nanos();
            let expected = u32::try_from((count - 1) * count / 2).unwrap();
            assert_eq!(result.as_number(), Some(f64::from(expected)));
            println!(
                "{count},{repeat},{create},{hit},{miss},{outer},{access},{parse},{compile},{evaluate}"
            );
        }
    }
}

fn measure(count: usize, mut work: impl FnMut(usize)) -> u128 {
    let start = Instant::now();
    for i in 0..count {
        work(i);
    }
    start.elapsed().as_nanos() / count as u128
}

fn script_source(count: usize) -> String {
    let mut source = String::from("(function(){");
    for i in 0..count {
        writeln!(source, "let binding_{i:05} = {i};").unwrap();
    }
    source.push_str("function read(){let sum=0;");
    for i in 0..count {
        writeln!(source, "sum += binding_{i:05};").unwrap();
    }
    source.push_str("return sum;} return read();})()");
    source
}
