<script lang="ts">
	import { getStatusMeta } from '$lib/api';
	import { formatDuration } from '$lib/time';
	import type { StatusMeta } from '$lib/types';
	import Card from '$lib/components/Card.svelte';
	import { Activity, CircleCheck, CircleX, Clock, RefreshCw, Users } from '@lucide/svelte';

	let status = $state<StatusMeta | null>(null);
	let lastText = $state('—');
	let nextText = $state('—');
	let uptimeText = $state('—');

	$effect(() => {
		getStatusMeta()
			.then((meta) => (status = meta))
			.catch(() => {});
	});

	$effect(() => {
		if (!status) return;

		const tick = () => {
			if (!status) return;
			lastText = `${formatDuration(status.lastUpdate)} ago`;
			nextText = `in ${formatDuration(status.lastUpdate + status.nextUpdate, true)}`;
			uptimeText = formatDuration(status.uptime);

			if (nextText === 'in 0 seconds') {
				clearInterval(interval);
				setTimeout(() => location.reload(), 5000);
			}
		};

		tick();
		const interval = setInterval(tick, 500);
		return () => clearInterval(interval);
	});

	const entries = $derived(Object.entries(status?.instances ?? {}));
	const upCount = $derived(entries.filter(([, info]) => info.up).length);
</script>

<svelte:head>
	<title>Logs | Status</title>
	<meta name="description" content="Status of instances" />
</svelte:head>

<div class="mx-auto max-w-5xl px-4 py-16 sm:px-6">
	<div class="mb-8 text-center">
		<h1 class="text-3xl font-extrabold tracking-tight text-fg">Instance Status</h1>
		<p class="mt-2 text-sm text-fg-muted">
			{upCount} of {entries.length} tracked instances are currently reachable.
		</p>
	</div>

	<div class="mb-8 grid gap-3 sm:grid-cols-3">
		<Card class="flex items-center gap-3 p-4!">
			<Clock class="h-5 w-5 shrink-0 text-accent" />
			<div class="min-w-0">
				<p class="text-xs text-fg-subtle">Last update</p>
				<p class="truncate text-sm font-semibold text-fg">{lastText}</p>
			</div>
		</Card>
		<Card class="flex items-center gap-3 p-4!">
			<RefreshCw class="h-5 w-5 shrink-0 text-accent" />
			<div class="min-w-0">
				<p class="text-xs text-fg-subtle">Next update</p>
				<p class="truncate text-sm font-semibold text-fg">{nextText}</p>
			</div>
		</Card>
		<Card class="flex items-center gap-3 p-4!">
			<Activity class="h-5 w-5 shrink-0 text-accent" />
			<div class="min-w-0">
				<p class="text-xs text-fg-subtle">Uptime</p>
				<p class="truncate text-sm font-semibold text-fg">{uptimeText}</p>
			</div>
		</Card>
	</div>

	<div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
		{#each entries as [name, info] (name)}
			<Card class="p-4!">
				<div class="flex items-start justify-between gap-2">
					<div class="min-w-0">
						<a
							href={`https://${name}`}
							class="block truncate font-semibold text-fg hover:text-accent"
						>
							{name}
						</a>
						{#if info.maintainer}
							<a
								href={`https://twitch.tv/${info.maintainer}`}
								class="text-xs text-fg-subtle hover:text-fg-muted"
							>
								by {info.maintainer}
							</a>
						{/if}
					</div>
					{#if info.up}
						<CircleCheck class="h-5 w-5 shrink-0 text-up" />
					{:else}
						<CircleX class="h-5 w-5 shrink-0 text-down" />
					{/if}
				</div>
				<div class="mt-3 flex items-center justify-between text-xs">
					<span class={info.up ? 'font-semibold text-up' : 'font-semibold text-down'}>
						{info.up ? 'UP' : 'DOWN'}
					</span>
					{#if info.up}
						<span class="flex items-center gap-1 text-fg-subtle">
							<Users class="h-3.5 w-3.5" />
							{new Intl.NumberFormat().format(info.channels)}
						</span>
					{/if}
				</div>
			</Card>
		{/each}
	</div>
</div>
