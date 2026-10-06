use crate::{Context, TestAction, js_string, run_test_actions_with};

fn limited(iterations: u64, bytes: u64) -> Context {
    let mut context = Context::default();
    context
        .runtime_limits_mut()
        .set_native_iteration_limit(iterations);
    context
        .runtime_limits_mut()
        .set_native_allocation_limit(bytes);
    context
}

#[test]
fn huge_sparse_receivers_fail_without_large_allocations_or_traversals() {
    let mut context = limited(32, 1024);
    run_test_actions_with(
        [TestAction::run(
            "function limited(f) {try {f();return false} catch(e) {return e instanceof RangeError}}\n\
         var huge={length:Number.MAX_SAFE_INTEGER};var sparse={length:4294967295};",
        )],
        &mut context,
    );
    for code in [
        "Array.prototype.join.call(huge)",
        "Array.prototype.flat.call(huge)",
        "Array.prototype.flatMap.call(huge, x=>[x])",
        "Array.prototype.toLocaleString.call(huge)",
        "Array.prototype.sort.call(huge)",
        "Array.prototype.forEach.call(huge,()=>{})",
        "Array.prototype.every.call(huge,()=>true)",
        "Array.prototype.some.call(huge,()=>false)",
        "Array.prototype.map.call(sparse,x=>x)",
        "Array.prototype.filter.call(huge,()=>true)",
        "Array.prototype.reduce.call(huge,()=>0,0)",
        "Array.prototype.reduceRight.call(huge,()=>0,0)",
        "Array.prototype.reduce.call(huge,()=>0)",
        "Array.prototype.reduceRight.call(huge,()=>0)",
        "Array.prototype.indexOf.call(huge,0)",
        "Array.prototype.lastIndexOf.call(huge,0)",
        "Array.prototype.includes.call(huge,0)",
        "Array.prototype.find.call(huge,()=>false)",
        "Array.prototype.findIndex.call(huge,()=>false)",
        "Array.prototype.findLast.call(huge,()=>false)",
        "Array.prototype.findLastIndex.call(huge,()=>false)",
        "Array.prototype.reverse.call(huge)",
        "Array.prototype.toReversed.call(sparse)",
        "Array.prototype.shift.call(huge)",
        "Array.prototype.unshift.call(sparse,0)",
        "Array.prototype.fill.call(huge,0)",
        "Array.prototype.slice.call(sparse)",
        "Array.prototype.splice.call(sparse,0)",
        "Array.prototype.toSpliced.call(sparse,0,0)",
        "Array.prototype.copyWithin.call(huge,0,1)",
        "Array.prototype.toSorted.call(sparse)",
        "Array.prototype.with.call(sparse,0,1)",
        "Array.from.call(function(){},huge)",
        "Array.prototype.concat.call({length:Number.MAX_SAFE_INTEGER,[Symbol.isConcatSpreadable]:true})",
    ] {
        run_test_actions_with(
            [
                TestAction::assert(format!("limited(()=>{{{code}}})")),
                TestAction::assert_eq("[1,2].join()", js_string!("1,2")),
            ],
            &mut context,
        );
    }
    run_test_actions_with(
        [
            TestAction::assert(
                "limited(()=>Array.from({[Symbol.iterator](){return {next(){return {value:1,done:false}}}}}))",
            ),
            TestAction::assert_eq("[1,2].join()", js_string!("1,2")),
        ],
        &mut context,
    );
}

#[test]
fn native_work_shares_budgets_through_getters_callbacks_species_and_conversions() {
    let mut context = limited(8, 1024);
    run_test_actions_with(
        [
            TestAction::run(
                "function limited(f){try{f();return false}catch(e){return e instanceof RangeError}}",
            ),
            TestAction::assert(
                "limited(()=>[1,2,3].map(()=>Array.prototype.flat.call({length:3})))",
            ),
            TestAction::assert(
                "limited(()=>Array.prototype.join.call({length:2,get 0(){return Array.prototype.flat.call({length:8})}}))",
            ),
            TestAction::assert(
                "limited(()=>Array.prototype.flat.call({get length(){Array.prototype.flat.call({length:8});return 1},0:1}))",
            ),
            TestAction::assert(
                "limited(()=>[1].join({toString(){Array.prototype.flat.call({length:8});return ','}}))",
            ),
            TestAction::assert(
                "limited(()=>{const a=[1];a.constructor={[Symbol.species]:function(){Array.prototype.flat.call({length:8})}};a.flat()})",
            ),
            TestAction::assert("limited(()=>[1,2,3].flatMap(()=>[1,2]))"),
            TestAction::assert(
                "limited(()=>[3,2,1].sort(()=>{Array.prototype.flat.call({length:8});return 0}))",
            ),
            TestAction::assert(
                "limited(()=>[1,2].map(()=>{try{Array.prototype.flat.call({length:7,get 0(){return Array.prototype.flat.call({length:6})}})}catch{};return 1}))",
            ),
            TestAction::assert_eq("[1,2].map(x=>x*2).join()", js_string!("2,4")),
        ],
        &mut context,
    );
}

#[test]
fn native_work_exact_boundaries_ordering_and_exception_cleanup() {
    let mut context = limited(3, 1024);
    run_test_actions_with(
        [
            TestAction::assert_eq(
                "Array.prototype.flat.call({length:3,0:1,2:3}).join()",
                js_string!("1,3"),
            ),
            TestAction::assert(
                "(()=>{try{Array.prototype.flat.call({length:4});return false}catch(e){return e instanceof RangeError}})()",
            ),
            TestAction::assert_eq(
                "Array.prototype.findIndex.call({length:Number.MAX_SAFE_INTEGER},()=>true)",
                0,
            ),
            TestAction::assert(
                "Array.prototype.includes.call({length:Number.MAX_SAFE_INTEGER},undefined)",
            ),
            TestAction::assert_eq(
                "Array.prototype.fill.call({length:Number.MAX_SAFE_INTEGER},7,0,1)[0]",
                7,
            ),
            TestAction::assert(
                "(()=>{const marker={};for(let i=0;i<10;i++){try{Array.prototype.join.call({length:1,get 0(){throw marker}})}catch(e){if(e!==marker)return false}}return true})()",
            ),
            TestAction::assert_eq(
                "(()=>{let log='';const o={get length(){log+='L';return 2},get 0(){log+='A';return 'x'},get 1(){log+='B';return 'y'}};const sep={toString(){log+='S';return '|'}};const r=Array.prototype.join.call(o,sep);return log+':'+r})()",
                js_string!("LSAB:x|y"),
            ),
            TestAction::assert_eq(
                "Array.prototype.join.call({length:3,0:'\\ud800',2:'\\udc00'},'')",
                js_string!(&[0xd800u16, 0xdc00u16]),
            ),
            TestAction::assert_eq("[1,2].join()", js_string!("1,2")),
        ],
        &mut context,
    );
    let mut context = limited(10, 12);
    run_test_actions_with(
        [
            TestAction::assert_eq("['abc'].join()", js_string!("abc")),
            TestAction::assert(
                "(()=>{try{['abcd'].join();return false}catch(e){return e instanceof RangeError}})()",
            ),
            TestAction::assert_eq("['abc'].join()", js_string!("abc")),
            TestAction::assert(
                "(()=>{try{['x','x'].join('abcdefgh');return false}catch(e){return e instanceof RangeError}})()",
            ),
            TestAction::assert(
                "(()=>{try{[2,1].sort();return false}catch(e){return e instanceof RangeError}})()",
            ),
        ],
        &mut context,
    );
}

#[test]
fn native_work_closes_iterators_and_restores_state_after_unwind() {
    let mut context = limited(3, 1024);
    run_test_actions_with(
        [
            TestAction::assert(
                "(()=>{let closed=0;const it={[Symbol.iterator](){return {next(){return {value:1,done:false}},return(){closed++;return {}}}}};try{Array.from(it);return false}catch(e){return e instanceof RangeError&&closed===1}})()",
            ),
            TestAction::assert_eq("[1,2].join()", js_string!("1,2")),
        ],
        &mut context,
    );
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let work = context.enter_native_work().unwrap();
        work.iterations(3).unwrap();
        panic!("test unwind");
    }));
    assert!(result.is_err());
    assert_eq!(context.vm.native_work.get().depth, 0);
    let work = context.enter_native_work().unwrap();
    assert!(work.iterations(3).is_ok());
    assert!(work.step().is_err());
    drop(work);
    context.runtime_limits_mut().set_native_iteration_limit(0);
    run_test_actions_with(
        [
            TestAction::assert_eq("[].join()", js_string!()),
            TestAction::assert(
                "(()=>{try{[1].join();return false}catch(e){return e instanceof RangeError}})()",
            ),
        ],
        &mut context,
    );
}

#[test]
fn native_work_admits_array_truncation_before_changing_length() {
    let mut context = Context::default();
    run_test_actions_with([TestAction::run("var victim=[1,2,3]")], &mut context);
    context.runtime_limits_mut().set_native_iteration_limit(0);
    context.runtime_limits_mut().set_native_allocation_limit(0);
    run_test_actions_with(
        [
            TestAction::assert(
                "(()=>{try{Array.from.call(function(){return victim},{length:0});return false}catch(e){return e instanceof RangeError}})()",
            ),
            TestAction::assert("victim.length===3 && victim[2]===3"),
            TestAction::assert(
                "(()=>{try{Array.of.call(function(){return victim});return false}catch(e){return e instanceof RangeError}})()",
            ),
            TestAction::assert("victim.length===3 && victim[2]===3"),
        ],
        &mut context,
    );
    context.runtime_limits_mut().set_native_iteration_limit(100);
    context
        .runtime_limits_mut()
        .set_native_allocation_limit(1024);
    run_test_actions_with(
        [
            TestAction::assert_eq(
                "Array.from.call(function(){return victim},{length:0}).length",
                0,
            ),
            TestAction::assert("victim.length===0 && !(0 in victim)"),
            TestAction::assert_eq("[1,2].join()", js_string!("1,2")),
        ],
        &mut context,
    );
}
