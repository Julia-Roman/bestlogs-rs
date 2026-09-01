<script lang="ts">
	import { Accordion } from 'bits-ui';
	import { ChevronDown } from '@lucide/svelte';
	import { resolve } from '$app/paths';
	import Card from '$lib/components/Card.svelte';

	const faqs = [
		{
			value: 'open-source',
			question: 'Open source?',
			answer: 'faq-open-source'
		},
		{
			value: 'error',
			question: 'Received an error?',
			answer: 'faq-error'
		},
		{
			value: 'no-logs',
			question: 'Why are there no logs for my channel?',
			answer: 'faq-no-logs'
		}
	];
</script>

<svelte:head>
	<title>Logs | FAQ</title>
	<meta name="description" content="Frequently Asked Questions" />
</svelte:head>

<div class="mx-auto max-w-2xl px-4 py-16 sm:px-6">
	<div class="mb-10 flex flex-col items-center gap-4 text-center">
		<img src="/dankSpin.gif" alt="dankSpin" class="h-20 w-20" />
		<h1 class="text-3xl font-extrabold tracking-tight text-fg">Frequently Asked Questions</h1>
	</div>

	<Card class="p-2 sm:p-3">
		<Accordion.Root type="single" class="flex flex-col divide-y divide-line">
			{#each faqs as faq (faq.value)}
				<Accordion.Item value={faq.value} class="py-1">
					<Accordion.Header>
						<Accordion.Trigger
							class="group flex w-full items-center justify-between gap-4 rounded-lg px-3 py-3 text-left text-sm font-semibold text-fg transition hover:bg-overlay"
						>
							{faq.question}
							<ChevronDown
								class="h-4 w-4 shrink-0 text-fg-subtle transition-transform group-data-[state=open]:rotate-180"
							/>
						</Accordion.Trigger>
					</Accordion.Header>
					<Accordion.Content
						class="overflow-hidden px-3 pb-4 text-sm leading-relaxed text-fg-muted"
					>
						{#if faq.answer === 'faq-open-source'}
							Yes, the code is available on
							<a
								href="https://github.com/Julia-Roman/bestlogs-rs"
								class="font-medium text-link hover:text-link-hover"
							>
								GitHub
							</a>.
						{:else if faq.answer === 'faq-error'}
							You either misspelled the user or statistics were not found for the specified channel.
							In any case, you can
							<a href={resolve('/contact')} class="font-medium text-link hover:text-link-hover"
								>contact</a
							>
							me or report the error on
							<a
								href="https://github.com/Julia-Roman/bestlogs-rs/issues"
								class="font-medium text-link hover:text-link-hover"
							>
								GitHub
							</a>.
						{:else}
							You should contact the maintainer of an available instance and ask them to add your
							channel. Check the
							<a href={resolve('/status')} class="font-medium text-link hover:text-link-hover"
								>Status</a
							>
							section for more information about the instances.
						{/if}
					</Accordion.Content>
				</Accordion.Item>
			{/each}
		</Accordion.Root>
	</Card>
</div>
