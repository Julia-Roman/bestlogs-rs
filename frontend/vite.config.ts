import tailwindcss from '@tailwindcss/vite';
import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [
		tailwindcss(),
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},

			// Static SPA build: every route is client-rendered and fetches its data
			// from the Rust backend at runtime, so the build output is embedded
			// as-is into the server binary (no prerendering, no backend needed to build).
			adapter: adapter({
				fallback: 'index.html'
			})
		})
	],
	server: {
		// During `npm run dev`, forward every backend path to a locally running
		// `cargo run` (default port 2028) so the SPA can be exercised without a build.
		proxy: {
			'/meta': 'http://127.0.0.1:2028',
			// regex form (leading ^) so the bare `/api` docs page still hits the SvelteKit route
			'^/api/.+': 'http://127.0.0.1:2028',
			'/rdr': 'http://127.0.0.1:2028',
			'/list': 'http://127.0.0.1:2028',
			'/channel': 'http://127.0.0.1:2028',
			'/channelid': 'http://127.0.0.1:2028',
			'/namehistory': 'http://127.0.0.1:2028',
			'/rm': 'http://127.0.0.1:2028',
			'/recent-messages': 'http://127.0.0.1:2028',
			'/instances': 'http://127.0.0.1:2028',
			'/channels': 'http://127.0.0.1:2028',
			'/health': 'http://127.0.0.1:2028'
		}
	}
});
