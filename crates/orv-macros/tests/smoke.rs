use orv_macros::orv;

#[test]
fn macro_compiles() {
    orv! {
        hello world
    };
}
