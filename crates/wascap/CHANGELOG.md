# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

<csr-id-f5459155f3b96aa67742a8c62eb286cc06885855/>
<csr-id-859b0baeff818a1af7e1824cbb80510669bdc976/>

### Documentation

 - <csr-id-20ffecb027c225fb62d60b584d6b518aff4ceb51/> update wash URLs

### Refactor

 - <csr-id-f5459155f3b96aa67742a8c62eb286cc06885855/> convert lattice-control provider to bindgen
   The `lattice-control` provider (AKA `lattice-controller`) enables
   easy (if not somewhat meta) control of a wasmcloud lattice, using the
   `wasmcloud-control-interface` crate.
   
   While in the past this provider was powered by Smithy contracts, in
   the WIT-ified future we must convert that contract to an WIT-ified
   interface which is backwards compatible with the smithy interface.
   
   This commit converts the `lattice-control` provider to use WIT-ified
   interfaces (rather than Smithy-based interfaces) and `provider-wit-bindgen`.

### Chore

 - <csr-id-90d7c48a46e112ab884d9836bfc25c1de5570fee/> add changelogs for wash

### Chore

 - <csr-id-859b0baeff818a1af7e1824cbb80510669bdc976/> add changelogs for host

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 4 commits contributed to the release over the course of 44 calendar days.
 - 50 days passed between releases.
 - 4 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Add changelogs for wash ([`90d7c48`](https://github.com/connorsmith256/wasmcloud/commit/90d7c48a46e112ab884d9836bfc25c1de5570fee))
    - Add changelogs for host ([`859b0ba`](https://github.com/connorsmith256/wasmcloud/commit/859b0baeff818a1af7e1824cbb80510669bdc976))
    - Convert lattice-control provider to bindgen ([`f545915`](https://github.com/connorsmith256/wasmcloud/commit/f5459155f3b96aa67742a8c62eb286cc06885855))
    - Update wash URLs ([`20ffecb`](https://github.com/connorsmith256/wasmcloud/commit/20ffecb027c225fb62d60b584d6b518aff4ceb51))
</details>

## v0.12.0 (2023-11-09)

<csr-id-9c8abf3dd1a942f01a70432abb2fb9cfc4d48914/>
<csr-id-ee9d552c7ea1c017d8aa646f64002a85ffebefb8/>
<csr-id-9de9ae3de8799661525b2458303e72cd24cd666f/>
<csr-id-0b59721367d138709b58fa241cdadd4f585203ac/>
<csr-id-171214d4bcffddb9a2a37c2a13fcbed1ec43fd31/>

### Chore

 - <csr-id-9c8abf3dd1a942f01a70432abb2fb9cfc4d48914/> address clippy issues
 - <csr-id-ee9d552c7ea1c017d8aa646f64002a85ffebefb8/> address `clippy` warnings in workspace
 - <csr-id-9de9ae3de8799661525b2458303e72cd24cd666f/> integrate `provider-archive` into the workspace
 - <csr-id-0b59721367d138709b58fa241cdadd4f585203ac/> integrate `wascap` into the workspace

### Refactor

 - <csr-id-171214d4bcffddb9a2a37c2a13fcbed1ec43fd31/> use `OnceLock` to remove `once-cell`
   This commit removes the use of `once-cell` in favor of `std::sync::OnceLock`

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 7 commits contributed to the release over the course of 22 calendar days.
 - 5 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Address clippy issues ([`9c8abf3`](https://github.com/connorsmith256/wasmcloud/commit/9c8abf3dd1a942f01a70432abb2fb9cfc4d48914))
    - Use `OnceLock` to remove `once-cell` ([`171214d`](https://github.com/connorsmith256/wasmcloud/commit/171214d4bcffddb9a2a37c2a13fcbed1ec43fd31))
    - Merge pull request #762 from rvolosatovs/merge/wascap ([`89570cc`](https://github.com/connorsmith256/wasmcloud/commit/89570cc8d7ac7fbf6acd83fdf91f2ac8014d0b77))
    - Address `clippy` warnings in workspace ([`ee9d552`](https://github.com/connorsmith256/wasmcloud/commit/ee9d552c7ea1c017d8aa646f64002a85ffebefb8))
    - Integrate `provider-archive` into the workspace ([`9de9ae3`](https://github.com/connorsmith256/wasmcloud/commit/9de9ae3de8799661525b2458303e72cd24cd666f))
    - Integrate `wascap` into the workspace ([`0b59721`](https://github.com/connorsmith256/wasmcloud/commit/0b59721367d138709b58fa241cdadd4f585203ac))
    - Add 'crates/wascap/' from commit '6dd214c2ea3befb5170d5a711a2eef0f3d14cc09' ([`260ffb0`](https://github.com/connorsmith256/wasmcloud/commit/260ffb029f05b8a6b6f9dcbf6870e281569694c2))
</details>

