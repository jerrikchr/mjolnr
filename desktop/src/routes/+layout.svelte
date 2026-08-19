<script lang="ts">
	import '../app.css';
	import { Toaster } from '$lib/components/ui/sonner';
	import { ModeWatcher } from 'mode-watcher';
	import type { Snippet } from 'svelte';

	interface Props { children?: Snippet }

	let { children }: Props = $props();
</script>

<ModeWatcher />
<div class="h-screen w-screen overflow-hidden bg-background text-foreground">{@render children?.()}</div>
<Toaster richColors />
