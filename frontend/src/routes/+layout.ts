// Every route is prerendered to its own HTML shell, but never server-rendered:
// pages fetch their data from the Rust backend at runtime, so the static build
// never needs a backend to exist.
export const ssr = false;
export const prerender = true;
