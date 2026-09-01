// Fully client-rendered SPA: every page fetches its own data at runtime from
// the Rust backend, so the static build never needs a backend to exist.
export const ssr = false;
export const prerender = false;
