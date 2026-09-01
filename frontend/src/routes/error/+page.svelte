<script lang="ts">
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import Card from '$lib/components/Card.svelte';
	import { TriangleAlert } from '@lucide/svelte';

	const code = $derived(page.url.searchParams.get('code') ?? '500');
	const message = $derived(page.url.searchParams.get('message') ?? 'Something went wrong');
</script>

<svelte:head>
	<title>Logs | Error</title>
	<meta name="description" content={`Error ${code} | ${message}`} />
</svelte:head>

<div class="mx-auto flex max-w-md flex-col items-center px-4 py-24 text-center sm:px-6">
	<Card class="flex w-full flex-col items-center gap-4 border-down/30 bg-down/5">
		<div class="flex h-14 w-14 items-center justify-center rounded-full bg-down/15 text-down">
			<TriangleAlert class="h-7 w-7" />
		</div>
		<div>
			<p class="text-sm font-semibold tracking-wide text-down uppercase">Error {code}</p>
			<h1 class="mt-1 text-xl font-bold text-fg">{message}</h1>
		</div>
		<img
			src="/FeelsDankMan.png"
			draggable="false"
			title="FeelsDankMan"
			alt="FeelsDankMan"
			class="h-24 w-24"
		/>
		<a
			href={resolve('/')}
			class="mt-2 inline-flex items-center gap-2 rounded-lg border border-line px-4 py-2 text-sm font-semibold text-fg-muted transition hover:bg-overlay"
		>
			Back home
		</a>
	</Card>
</div>
