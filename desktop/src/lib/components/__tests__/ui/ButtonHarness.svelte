<script lang="ts">
  import { Button } from '$lib/components/ui/button';

  let { onAction }: { onAction: () => void } = $props();
</script>

<Button onclick={onAction}>Approve once</Button>
