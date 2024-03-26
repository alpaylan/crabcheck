use cpu_time::ProcessTime;

pub fn with_time<T>(f: Box<dyn FnOnce() -> T + '_>) -> (T, std::time::Duration) {
    let start = ProcessTime::now();
    let result = f();
    let duration = start.elapsed();
    (result, duration)
}