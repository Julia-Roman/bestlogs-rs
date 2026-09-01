<script lang="ts">
	import type { Snippet } from 'svelte';

	let { class: className = '', children }: { class?: string; children: Snippet } = $props();
</script>

<div
	class="rounded-2xl border border-line bg-surface/70 p-6 shadow-lg shadow-black/5 backdrop-blur-sm dark:shadow-black/20 {className}"
>
	{@render children()}
</div>
