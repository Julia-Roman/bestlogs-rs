<script lang="ts">
	import { Avatar } from 'bits-ui';
	import { getContactMeta } from '$lib/api';
	import type { ContactMeta } from '$lib/types';
	import Card from '$lib/components/Card.svelte';
	import BrandIcon from '$lib/components/BrandIcon.svelte';
	import { User } from '@lucide/svelte';

	let contact = $state<ContactMeta | null>(null);

	$effect(() => {
		getContactMeta()
			.then((meta) => (contact = meta))
			.catch(() => {});
	});
</script>

<svelte:head>
	<title>Logs | Contact</title>
	<meta name="description" content="Contact with @ZonianMidian" />
</svelte:head>

<div class="mx-auto max-w-3xl px-4 py-16 sm:px-6">
	<h1 class="mb-10 text-center text-3xl font-extrabold tracking-tight text-fg">Contact</h1>

	<div class="grid gap-5 sm:grid-cols-2">
		{#if contact?.creator}
			<Card class="flex flex-col items-center gap-3 text-center">
				<Avatar.Root class="h-24 w-24 overflow-hidden rounded-full ring-2 ring-brand-500/40">
					<Avatar.Image
						src={contact.creator.avatar}
						alt={contact.creator.name}
						class="h-full w-full object-cover"
					/>
					<Avatar.Fallback
						class="flex h-full w-full items-center justify-center bg-overlay text-fg-subtle"
					>
						<User class="h-8 w-8" />
					</Avatar.Fallback>
				</Avatar.Root>
				<h2 class="text-xl font-bold text-fg">{contact.creator.name}</h2>
				<div class="flex items-center gap-4 text-fg-muted">
					<a href="https://x.com/ZonianMidian" aria-label="X" class="transition hover:text-accent">
						<BrandIcon name="x" />
					</a>
					<a
						href="https://twitch.tv/ZonianMidian"
						aria-label="Twitch"
						class="transition hover:text-accent"
					>
						<BrandIcon name="twitch" />
					</a>
					<a
						href="https://github.com/ZonianMidian"
						aria-label="GitHub"
						class="transition hover:text-accent"
					>
						<BrandIcon name="github" />
					</a>
					<a
						href="https://discord.gg/rMjftZx8Ex"
						aria-label="Discord"
						class="transition hover:text-accent"
					>
						<BrandIcon name="discord" />
					</a>
				</div>
				<p class="text-sm text-fg-muted">Contact me for questions or suggestions :P</p>
			</Card>
		{/if}

		{#if contact?.maintainer?.name}
			<Card class="flex flex-col items-center gap-3 text-center">
				<Avatar.Root class="h-24 w-24 overflow-hidden rounded-full ring-2 ring-brand-500/40">
					<Avatar.Image
						src={contact.maintainer.avatar}
						alt={contact.maintainer.name}
						class="h-full w-full object-cover"
					/>
					<Avatar.Fallback
						class="flex h-full w-full items-center justify-center bg-overlay text-fg-subtle"
					>
						<User class="h-8 w-8" />
					</Avatar.Fallback>
				</Avatar.Root>
				<h2 class="text-xl font-bold text-fg">{contact.maintainer.name}</h2>
				<a
					href={`https://twitch.tv/${contact.maintainer.name}`}
					aria-label="Twitch"
					class="text-fg-muted transition hover:text-accent"
				>
					<BrandIcon name="twitch" />
				</a>
				{#if contact.maintainer.message}
					<p class="text-sm text-fg-muted">{contact.maintainer.message}</p>
				{/if}
			</Card>
		{/if}
	</div>
</div>
