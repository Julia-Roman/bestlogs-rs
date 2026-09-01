<script lang="ts">
	import '../app.css';
	import Navbar from '$lib/components/Navbar.svelte';
	import Footer from '$lib/components/Footer.svelte';
	import { loadMeta, metaState } from '$lib/meta.svelte';
	import { initTheme } from '$lib/theme.svelte';

	let { children } = $props();

	loadMeta();
	initTheme();

	$effect(() => {
		const umami = metaState.value?.umami;
		if (!umami || document.querySelector('script[data-umami-injected]')) return;

		const script = document.createElement('script');
		script.src = `${umami.url}/script.js`;
		script.defer = true;
		script.dataset.websiteId = umami.id;
		script.dataset.umamiInjected = 'true';
		document.head.appendChild(script);
	});
</script>

<svelte:head>
	<link rel="icon" href="/favicon.ico" />
	<title>Best Logs</title>
</svelte:head>

<div class="flex min-h-screen flex-col bg-canvas font-sans text-fg antialiased">
	<Navbar />
	<main class="flex-1 pt-16">
		{@render children()}
	</main>
	<Footer commit={metaState.value?.commit ?? null} />
</div>
