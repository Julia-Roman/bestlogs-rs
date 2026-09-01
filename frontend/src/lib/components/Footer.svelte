<script lang="ts">
	import { GitCommitHorizontal } from '@lucide/svelte';

	let { commit }: { commit: string | null } = $props();

	const REPO_URL = 'https://github.com/Julia-Roman/bestlogs-rs';
</script>

<footer class="border-t border-line py-8 text-center">
	{#if commit}
		<a
			href={REPO_URL}
			class="inline-flex items-center gap-1.5 text-xs font-medium tracking-wide text-fg-subtle transition hover:text-fg-muted"
		>
			<GitCommitHorizontal class="h-3.5 w-3.5" />
			{commit}
		</a>
	{/if}
</footer>
