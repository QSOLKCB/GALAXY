// SPDX-License-Identifier: Apache-2.0
//! Print the demo's five rotation-curve predictions without a browser or Python.
use galaxy_sampler::physics::{velocity_kms, Parameters};
fn main() {
    println!("radius_kpc,baryons_kms,nfw_kms,burkert_kms,mond_rar_kms,uff_empirical_kms");
    for radius in [0.5, 1.0, 2.0, 5.0, 8.0, 12.0] {
        print!("{radius:.1}");
        for model in 1..=5 {
            let p = Parameters { model, ..Parameters::default() };
            print!(",{:.9}", velocity_kms(radius, &p).expect("valid demo parameters"));
        }
        println!();
    }
}
