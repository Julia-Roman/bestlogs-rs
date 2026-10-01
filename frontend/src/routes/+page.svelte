<script lang="ts">
	import { loadMeta, metaState } from '$lib/meta.svelte';
	import Card from '$lib/components/Card.svelte';
	import { ArrowRight, ExternalLink } from '@lucide/svelte';
	import { resolve } from '$app/paths';

	loadMeta();
</script>

<svelte:head>
	<title>Best Logs</title>
	<meta name="description" content="Logs finder for Twitch" />
</svelte:head>

<div class="mx-auto max-w-5xl px-4 pt-16 pb-24 sm:px-6">
	<section class="flex flex-col items-center gap-6 text-center">
		<img src="/DankG.gif" alt="DankG" class="h-28 w-28" />
		<h1
			class="bg-linear-to-br from-fg via-accent to-brand-500 bg-clip-text text-4xl font-extrabold tracking-tight text-transparent sm:text-5xl"
		>
			Find any Twitch chat log
		</h1>
		<p class="max-w-lg text-balance text-fg-muted">
			Best Logs looks across every known logging instance and hands you back the best available
			result for a channel or user &mdash; nothing is stored here.
		</p>
		<div class="flex flex-wrap items-center justify-center gap-3">
			<a
				href={resolve('/api')}
				class="inline-flex items-center gap-2 rounded-lg bg-brand-600 px-4 py-2 text-sm font-semibold text-white transition hover:bg-brand-500"
			>
				Explore the API
				<ArrowRight class="h-4 w-4" />
			</a>
			<a
				href={resolve('/status')}
				class="inline-flex items-center gap-2 rounded-lg border border-line px-4 py-2 text-sm font-semibold text-fg-muted transition hover:bg-overlay"
			>
				Instance status
			</a>
		</div>
	</section>

	<section class="mt-16 grid gap-4 sm:grid-cols-2">
		<Card>
			<h2 class="text-lg font-bold text-fg">About</h2>
			<p class="mt-2 text-sm leading-relaxed text-fg-muted">
				This website does not store information of any kind &mdash; it only intends to collect
				public information about user logs in Twitch chats, and display them to visitors in a nice
				and clean way.
			</p>
		</Card>
		<Card>
			<h2 class="text-lg font-bold text-fg">Usage</h2>
			<p class="mt-2 text-sm leading-relaxed text-fg-muted">
				Go to the <a href={resolve('/api')} class="font-medium text-link hover:text-link-hover"
					>API</a
				>
				section for more details on how to use this service, including the channel mirror, redirects and
				recent-messages endpoints.
			</p>
		</Card>
	</section>

	<section class="mt-4">
		<Card>
			<h2 class="text-lg font-bold text-fg">Used Data &amp; APIs</h2>
			<p class="mt-2 text-sm text-fg-muted">
				The data is currently obtained from the following providers:
			</p>
			<div class="mt-4 flex flex-wrap gap-2">
				{#each metaState.value?.instances ?? [] as name (name)}
					<a
						href={`https://${name}`}
						class="inline-flex items-center gap-1.5 rounded-full border border-line bg-overlay px-3 py-1.5 text-xs font-medium text-fg-muted transition hover:border-brand-500/50 hover:text-fg"
					>
						{name}
						<ExternalLink class="h-3 w-3 opacity-60" />
					</a>
				{/each}
			</div>
		</Card>
	</section>
</div>
