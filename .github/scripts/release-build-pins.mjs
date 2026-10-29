#!/usr/bin/env node

import { appendFileSync, readFileSync } from 'node:fs';

const read = (path) => readFileSync(new URL(`../../${path}`, import.meta.url), 'utf8');
const rust = read('rust-toolchain.toml').match(/^channel = "(\d+\.\d+\.\d+)"$/m)?.[1];
const goVersion = (component) => read(`${component}/Dockerfile`).match(/^FROM golang:(\d+\.\d+\.\d+) AS builder$/m)?.[1];
const go = goVersion('runtime-operator');
if (!rust || !go || go !== goVersion('runtime-gateway')) {
  throw new Error('Rust must have an exact version, and both Go Dockerfiles must pin the same exact version');
}
if (!process.env.GITHUB_OUTPUT) throw new Error('GITHUB_OUTPUT is required');
appendFileSync(process.env.GITHUB_OUTPUT, `rust=${rust}\ngo=${go}\n`);
