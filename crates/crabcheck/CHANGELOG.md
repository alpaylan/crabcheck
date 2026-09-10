# Changelog

All notable changes to `crabcheck` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Changed
- Repository reorganised into a Cargo workspace. The 0.1.0 code (random
  testing plus coverage-based fault localization) now lives in
  `crates/crabcheck-locate`; `crates/crabcheck` is being rewritten around
  choice-tree shrinking, targeted mutation, and seed replay.
