//! The `pod-factlite` binary: serves [`factlite_pod::namespace`] over the
//! transport babashka asks for.

use babashka_pod::{Format, Pod};

fn main() {
    if !babashka_pod::is_pod_mode() {
        eprintln!("pod-factlite is a babashka pod; load it with babashka.pods/load-pod");
        std::process::exit(2);
    }
    if let Err(error) = Pod::new()
        .format(Format::TransitJson)
        .namespace(factlite_pod::namespace())
        .run()
    {
        eprintln!("pod-factlite: {error}");
        std::process::exit(1);
    }
}
