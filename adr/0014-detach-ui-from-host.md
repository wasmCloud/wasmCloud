# Detach UI from Host

| Status | Deciders | Date |
|--|--|--|
| accepted | wasmCloud Maintainers | 07-12-2023 |

## Context and Problem Statement

With the decision to transition the primary host to Rust, we have the ability to rethink the Washboard UI for the host. The current UI is attached directly to the OTP host, and is written in Elixir. This means that the UI can only be used when an OTP host is running, and that the UI is not portable to other hosts.

## Decision Drivers <!-- optional -->

* OTP host is being deprecated
* UI should be compatible with all hosts
* Community contributions to the UI should be easy
* UI should be easy to run anywhere

## Considered Options

* Static HTML/JS/CSS
* Rust based UIs (Yew, Seed, etc)
* Wash WASI Plugin
* Phoenix (Elixir) App
* Platform-agnostic App (Flutter, React Native, etc)
* Web wrapper App (Electron, Tauri, etc)

## Decision Outcome

The decision is to use a static HTML/JS/CSS UI. This will allow the UI to be run anywhere that can display HTML, and will allow the UI to be easily contributed to by the community. The UI will be written in React, Typescript, and Tailwind CSS. The build will be handled with Vite, though this is an implementation detail.

### UI Framework

There are a number of UI frameworks that could be used to build the UI. The most popular of these are React, Vue, and Svelte. Angular is not an option given the majority negative sentiment amongst the wider community. The decision to use React was made based on the fact that currently has the widest number of contributors and the largest ecosystem of libraries. Svelte was a close second, but the ecosystem is not as mature, and the community is not as large as React's.

### Styling and Component Library

For styling we wanted the most flexibility with as much core functionality covered as possible. For that reason we decided to use Radix-UI along with Tailwind CSS. This means that accessibility is a primary consideration while allowing for a large number of components to be used out of the box that can be styled to fit within the wasmCloud brand guidelines.

### Build tooling

The build tooling is not a primary consideration, but it is important to note that the build tooling should be as simple as possible. The decision to use Vite was made because it is a simple build tool that is easy to configure because of with opinionated defaults and has a large number of plugins available given that it is mostly built on top of Rollup.

## Pros and Cons of the Options

### Static HTML/JS/CSS

This consideration was not initially brought up in the RFC, but came out through the discussion of the other options. Many of the other options require some sort of HTML/JS/CSS UI to be built, and their benefits come only with how the project is distributed. Almost every device that can run a UI already has a browser installed, and the flexibility of where the UI is hosted cannot be understated.

### Rust based UIs (Yew, Seed, etc)

While these would give us the advantage of staying within the Rust ecosystem, it actually limits the amount of contributors that can work on the UI. These frameworks are not as widely used as React, and would require contributors—event those that already know Rust—to learn a new framework.

### Wash WASI Plugin

This option would allow us to keep on the bleeding edge of WASI and would allow us to make use of existing code. The downside is that it still requires an actual UI to be built so it only solves part of the problem.

### Phoenix (Elixir) App

Moving the the existing Elixir LiveView UI to a standalone app would mean less work building a new UI but the effort is not zero. The existing UI is coupled to a host and assumes a lot of functionality that is not accessible over the lattice. The burden of building and distributing an elixir app is also not trivial.

### Platform-agnostic App (Flutter, React Native, etc)

This option would allow us to build a UI that can be run on practically any platform with a screen. This benefit doesn't seem like such an advantage when most of the administration is going to take place from a laptop or desktop. In addition, this option would require us to learn a new framework and would limit the number of contributors.

### Web wrapper App (Electron, Tauri, etc)

A wrapper application would allow for the UI to be distributed as a binary but still requires that an HTML/JS/CSS UI be built. This isn't really an advantage when every device that can run the wrapper more than likely already has a web browser.

## Links

* [Original RFC](https://github.com/wasmCloud/wasmCloud/issues/321)
* [[ADR-0013] Transition Feature Focus to Rust](./0013-transition-feature-focus-to-rust.md)
* [React](https://reactjs.org/)
* [Vite](https://vitejs.dev/)
* [Tailwind CSS](https://tailwindcss.com/)
* [Radix-UI](https://radix-ui.com/)