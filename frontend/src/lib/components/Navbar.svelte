<script lang="ts">
	import { Collapsible } from 'bits-ui';
	import { Menu, Moon, Search, Sun, X } from '@lucide/svelte';
	import { resolve } from '$app/paths';
	import { themeState, toggleTheme } from '$lib/theme.svelte';

	let open = $state(false);
	let channel = $state('');
	let user = $state('');

	function submitSearch(event: SubmitEvent) {
		event.preventDefault();

		const cleanChannel = channel.trim();
		const cleanUser = user.trim();

		if (typeof window !== 'undefined' && window.umami) {
			window.umami.track('search', { channel: cleanChannel, user: cleanUser });
		}

		// /rdr/* is served by the Rust backend, not a SvelteKit route.
		const path = cleanUser
			? `/rdr/${encodeURIComponent(cleanChannel)}/${encodeURIComponent(cleanUser)}`
			: `/rdr/${encodeURIComponent(cleanChannel)}`;

		window.open(`${path}?pretty=true`, '_blank');
		open = false;
	}

	const links = [
		{ href: resolve('/api'), label: 'API' },
		{ href: resolve('/contact'), label: 'Contact' },
		{ href: resolve('/faq'), label: 'FAQ' },
		{ href: resolve('/status'), label: 'Status' }
	];
</script>

<Collapsible.Root bind:open>
	<header
		class="fixed inset-x-0 top-0 z-40 border-b border-line bg-canvas/80 backdrop-blur-md supports-backdrop-filter:bg-canvas/60"
	>
		<div class="mx-auto flex max-w-5xl items-center justify-between gap-4 px-4 py-3 sm:px-6">
			<a
				href={resolve('/')}
				class="flex items-center gap-2.5 font-extrabold tracking-tight text-fg"
			>
				<img src="/DankG.png" alt="" class="h-8 w-8" />
				<span>Best Logs</span>
			</a>

			<nav class="hidden items-center gap-6 md:flex">
				{#each links as link (link.href)}
					<a href={link.href} class="text-sm font-medium text-fg-muted transition hover:text-fg">
						{link.label}
					</a>
				{/each}
			</nav>

			<div class="hidden items-center gap-2 md:flex">
				<form onsubmit={submitSearch} class="flex items-center gap-2">
					<input
						type="search"
						placeholder="Channel"
						aria-label="Channel"
						bind:value={channel}
						required
						class="w-32 rounded-lg border border-line bg-surface px-3 py-1.5 text-sm text-fg outline-none placeholder:text-fg-subtle focus:border-brand-500/60 focus:ring-2 focus:ring-brand-500/30"
					/>
					<input
						type="search"
						placeholder="User"
						aria-label="User"
						bind:value={user}
						class="w-28 rounded-lg border border-line bg-surface px-3 py-1.5 text-sm text-fg outline-none placeholder:text-fg-subtle focus:border-brand-500/60 focus:ring-2 focus:ring-brand-500/30"
					/>
					<button
						type="submit"
						aria-label="Search"
						class="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-brand-600 text-white transition hover:bg-brand-500"
					>
						<Search class="h-4 w-4" />
					</button>
				</form>
				<button
					type="button"
					onclick={toggleTheme}
					aria-label="Toggle theme"
					class="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg border border-line text-fg-muted transition hover:bg-overlay hover:text-fg"
				>
					{#if themeState.value === 'dark'}
						<Sun class="h-4 w-4" />
					{:else}
						<Moon class="h-4 w-4" />
					{/if}
				</button>
			</div>

			<div class="flex items-center gap-2 md:hidden">
				<button
					type="button"
					onclick={toggleTheme}
					aria-label="Toggle theme"
					class="flex h-9 w-9 items-center justify-center rounded-lg border border-line text-fg-muted"
				>
					{#if themeState.value === 'dark'}
						<Sun class="h-4 w-4" />
					{:else}
						<Moon class="h-4 w-4" />
					{/if}
				</button>
				<Collapsible.Trigger
					class="flex h-9 w-9 items-center justify-center rounded-lg border border-line text-fg-muted"
					aria-label="Toggle navigation"
				>
					{#if open}
						<X class="h-5 w-5" />
					{:else}
						<Menu class="h-5 w-5" />
					{/if}
				</Collapsible.Trigger>
			</div>
		</div>

		<Collapsible.Content
			class="absolute inset-x-0 top-full overflow-hidden border-t border-line bg-canvas/95 backdrop-blur-md md:hidden"
		>
			<div class="flex flex-col gap-4 px-4 py-4 sm:px-6">
				<nav class="flex flex-col gap-1">
					{#each links as link (link.href)}
						<a
							href={link.href}
							onclick={() => (open = false)}
							class="rounded-lg px-3 py-2 text-sm font-medium text-fg-muted transition hover:bg-overlay hover:text-fg"
						>
							{link.label}
						</a>
					{/each}
				</nav>
				<form onsubmit={submitSearch} class="flex flex-col gap-2">
					<input
						type="search"
						placeholder="Channel"
						aria-label="Channel"
						bind:value={channel}
						required
						class="rounded-lg border border-line bg-surface px-3 py-2 text-sm text-fg outline-none placeholder:text-fg-subtle focus:border-brand-500/60 focus:ring-2 focus:ring-brand-500/30"
					/>
					<input
						type="search"
						placeholder="User"
						aria-label="User"
						bind:value={user}
						class="rounded-lg border border-line bg-surface px-3 py-2 text-sm text-fg outline-none placeholder:text-fg-subtle focus:border-brand-500/60 focus:ring-2 focus:ring-brand-500/30"
					/>
					<button
						type="submit"
						class="flex items-center justify-center gap-2 rounded-lg bg-brand-600 px-3 py-2 text-sm font-semibold text-white transition hover:bg-brand-500"
					>
						<Search class="h-4 w-4" />
						Search
					</button>
				</form>
			</div>
		</Collapsible.Content>
	</header>
</Collapsible.Root>
