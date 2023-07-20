use std::path::PathBuf;

// The following integration test uses insta snapshots of the
// tests/golden/custom-resource-definition.yaml to signal when something changes in our
// Rust CRD definitions.
//
// A common workflow:
//
// 1. Run the tests with `cargo test`.
// 2. If an insta snapshot error is found, note the line that has change
//    in the golden file.
// 3. Make the necessary changes to the bottlerocket-shadow helm chart
//    to correctly reflect the changes found via this insta test.
//    Because the Rust CRD definitions can't accept template strings
//    (example: the conversion webhook port field needs to be a number),
//    the generated Rust yaml is decoupled from the helm chart.
// 4. Run `cargo insta review` to accept the new changes into the snapshot.
// 5. Run `make check-crd-golden-diff` to ensure there are no hanging changes
//    to be made to the helm chart template.
// 6. Commit your changes and submit for review.

#[test]
fn test_generated_crds() {
    let path = PathBuf::from("./tests")
        .join("golden")
        .join("custom-resource-definition.yaml");

    let crds = std::fs::read_to_string(path).expect("Unable to read file");

    insta::assert_snapshot!(crds);
}
