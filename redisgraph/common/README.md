# waSCC Graph DB Common

This crate contains types and utility functions that are shared between a Graph DB capability provider and an actor consuming said provider. Ideally, _any_ graph db capability provider (e.g. Neo4j, RedisGraph, etc) should share the same set of common types and only differ in the implementation of the capability provider.

If, at some point, this set of common types becomes insufficient, then we should refactor these types rather than creating a new crate to support one-off graph database providers.
