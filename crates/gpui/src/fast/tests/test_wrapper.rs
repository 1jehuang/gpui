use gpui::{App, BackgroundExecutor, TestAppContext};
use rand::{RngCore, SeedableRng, rngs::StdRng};

macro_rules! check_hygiene {
    ($dispatcher:ident, $cx_0:ident) => {{
        let $dispatcher = 7;
        let $cx_0 = 13;
        assert_eq!($dispatcher + $cx_0, 20);
    }};
}

fn on_failure() {
    panic!("unexpected fixture failure")
}

#[gpui::test(seed = 27, retries = 1, on_failure = "on_failure")]
fn synchronous(cx: &mut TestAppContext, _: &mut App, mut rng: StdRng) {
    #![allow(unused_variables)]
    let intentionally_unused = 3;
    check_hygiene!(dispatcher, cx_0);
    assert_eq!(cx.test_function_name(), Some("synchronous"));
    assert_eq!(rng.next_u64(), StdRng::seed_from_u64(27).next_u64());
}

#[gpui::test(seeds(13, 19))]
async fn asynchronous(cx: &mut TestAppContext, other: &mut TestAppContext, _: BackgroundExecutor) {
    std::future::ready(()).await;
    assert_eq!(cx.test_function_name(), other.test_function_name());
    assert_eq!(cx.test_function_name(), Some("asynchronous"));
}

#[gpui::test]
#[should_panic(expected = "fixture sentinel")]
fn expected_panic() {
    panic!("fixture sentinel");
}

#[gpui::test]
#[ignore = "fixture checks wrapper attribute placement"]
fn ignored() {
    panic!("ignored fixture must never run");
}
