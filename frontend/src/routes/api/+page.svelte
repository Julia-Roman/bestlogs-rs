<script lang="ts">
	import Card from '$lib/components/Card.svelte';
	import {
		ArrowLeftRight,
		FileSearch,
		CornerDownRight,
		History,
		Server,
		LayoutGrid,
		HeartPulse,
		MessagesSquare
	} from '@lucide/svelte';
	import type { Component } from 'svelte';

	interface Param {
		name: string;
		desc: string;
	}

	interface Endpoint {
		icon: Component;
		title: string;
		path: string;
		description: string;
		params?: Param[];
		examples: { href: string; label: string }[];
	}

	const endpoints: Endpoint[] = [
		{
			icon: ArrowLeftRight,
			title: 'Mirror',
			path: 'any rustlog-compatible path',
			description:
				'Use any log instance endpoint and Best Logs resolves the best result for the channel-user combination. See the upstream docs for the full endpoint reference.',
			examples: [
				{ href: '/channel/forsen', label: '/channel/forsen' },
				{ href: '/channelid/22484632', label: '/channelid/22484632' },
				{ href: '/channel/forsen/user/nymn', label: '/channel/forsen/user/nymn' },
				{ href: '/channel/forsen/2024/1/1', label: '/channel/forsen/2024/1/1' },
				{ href: '/list?channel=forsen', label: '/list?channel=forsen' },
				{ href: 'https://logs.ivr.fi/docs', label: 'logs.ivr.fi/docs (full reference)' }
			]
		},
		{
			icon: FileSearch,
			title: 'API',
			path: 'api/:channel/:user',
			description: "Details for a user's available logs.",
			params: [
				{ name: ':channel', desc: 'A username (TenkSit) or an id (id:557479550)' },
				{ name: ':user (optional)', desc: 'A username (vicesmile) or an id (id:187698721)' },
				{ name: 'plain (bool)', desc: 'Get the instance link in plain text' },
				{ name: 'pretty (bool)', desc: 'Get the links using an enhanced version of the logs' }
			],
			examples: [
				{ href: '/api/orslok/Spanixbot', label: '/api/orslok/Spanixbot' },
				{ href: '/api/forsen/id:117691339', label: '/api/forsen/id:117691339' }
			]
		},
		{
			icon: CornerDownRight,
			title: 'Redirect',
			path: 'rdr/:channel/:user',
			description: "Simple redirect to an available instance for the user's logs.",
			params: [
				{ name: ':channel', desc: 'A username (NOTLUSHIN) or an id (id:410014058)' },
				{ name: ':user (optional)', desc: 'A username (Drapsnatt) or an id (id:43547909)' },
				{ name: 'pretty (bool)', desc: 'Get the links using an enhanced version of the logs' }
			],
			examples: [
				{ href: '/rdr/FapParaMoar/Rubius', label: '/rdr/FapParaMoar/Rubius' },
				{ href: '/rdr/ZonianMidian/id:570220755', label: '/rdr/ZonianMidian/id:570220755' }
			]
		},
		{
			icon: History,
			title: 'Name History',
			path: 'namehistory/:user',
			description: 'Obtains the history of usernames used by someone.',
			params: [{ name: ':user', desc: 'An id (596675864) or a username (login:xqc)' }],
			examples: [
				{ href: '/namehistory/93281234', label: '/namehistory/93281234' },
				{ href: '/namehistory/login:vei', label: '/namehistory/login:vei' }
			]
		},
		{
			icon: Server,
			title: 'Instances',
			path: 'instances',
			description: 'List of tracked logging instances.',
			examples: [{ href: '/instances', label: '/instances' }]
		},
		{
			icon: LayoutGrid,
			title: 'Channels',
			path: 'channels',
			description: 'List of unique channels across all instances.',
			examples: [{ href: '/channels', label: '/channels' }]
		},
		{
			icon: HeartPulse,
			title: 'Health',
			path: 'health',
			description: 'Health check for the API with basic data.',
			examples: [{ href: '/health', label: '/health' }]
		},
		{
			icon: MessagesSquare,
			title: 'Recent Messages',
			path: 'rm/:channel',
			description: 'Most complete history from a recent-messages instance.',
			params: [
				{ name: ':channel', desc: 'A username only (AlkalineXTw)' },
				{ name: 'hide_moderation_messages (bool)', desc: 'Omits CLEARCHAT and CLEARMSG messages' },
				{
					name: 'hide_moderated_messages (bool)',
					desc: 'Omits messages that were later deleted by a CLEARCHAT/CLEARMSG'
				},
				{
					name: 'clearchat_to_notice (bool)',
					desc: 'Converts CLEARCHAT messages into user-presentable NOTICE messages'
				},
				{ name: 'limit (number)', desc: 'Limit the number of messages returned' },
				{ name: 'before (number)', desc: 'Only messages received before this ms-epoch timestamp' },
				{ name: 'after (number)', desc: 'Only messages received after this ms-epoch timestamp' },
				{
					name: 'rm_only (bool)',
					desc: 'Only query recent-messages instances (faster, no history backfill)'
				}
			],
			examples: [{ href: '/rm/RyanPotat', label: '/rm/RyanPotat' }]
		}
	];
</script>

<svelte:head>
	<title>Logs | API</title>
	<meta name="description" content="Best Logs API" />
</svelte:head>

<div class="mx-auto max-w-4xl px-4 py-16 sm:px-6">
	<div class="mb-10 text-center">
		<h1 class="text-3xl font-extrabold tracking-tight text-fg">API Reference</h1>
		<p class="mt-2 text-sm text-fg-muted">
			Every endpoint Best Logs exposes, with example requests.
		</p>
	</div>

	<div class="flex flex-col gap-4">
		{#each endpoints as endpoint (endpoint.title)}
			<Card>
				<div class="flex items-start gap-4">
					<div
						class="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-brand-600/15 text-accent"
					>
						<endpoint.icon class="h-5 w-5" />
					</div>
					<div class="min-w-0 flex-1">
						<div class="flex flex-wrap items-center gap-2">
							<h2 class="text-lg font-bold text-fg">{endpoint.title}</h2>
							<code
								class="rounded-md border border-line bg-overlay px-2 py-0.5 text-xs text-accent"
							>
								{endpoint.path}
							</code>
						</div>
						<p class="mt-2 text-sm text-fg-muted">{endpoint.description}</p>

						{#if endpoint.params}
							<ul class="mt-4 space-y-1.5">
								{#each endpoint.params as param (param.name)}
									<li class="text-sm text-fg-muted">
										<span class="font-mono text-xs font-semibold text-fg">{param.name}</span>
										&mdash; {param.desc}
									</li>
								{/each}
							</ul>
						{/if}

						<div class="mt-4 flex flex-col gap-1">
							{#each endpoint.examples as example (example.href)}
								<!-- these are backend/external endpoints, not SvelteKit routes -->
								<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -->
								<a href={example.href} class="text-sm font-medium text-link hover:text-link-hover">
									{example.label}
								</a>
							{/each}
						</div>
					</div>
				</div>
			</Card>
		{/each}
	</div>
</div>
