use miol_macros::miol;

#[test]
fn macro_compiles() {
    let _result = miol! {
        hello world
    };
}
