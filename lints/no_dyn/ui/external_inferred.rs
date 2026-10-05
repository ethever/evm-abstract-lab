// edition:2024
// aux-build: dispatch_types.rs

extern crate dispatch_types;

use dispatch_types::Work;

fn main() {
    let action = dispatch_types::callback();
    let _ = action(1);

    let object = dispatch_types::erased();
    let _ = object.run();

    // The callee's dependency-owned signature forces a trait-object coercion.
    let _ = dispatch_types::accept(&1u8);

    // The source receiver is concrete; autoderef introduces its dyn target.
    let wrapper = dispatch_types::Wrapper::new(1);
    let _ = wrapper.run();

    // UFCS must also detect a virtual call whose return type is concrete.
    let _ = Work::run(dispatch_types::borrowed(&1u8));
}
