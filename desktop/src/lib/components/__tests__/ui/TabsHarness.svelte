<script lang="ts">
  import * as Tabs from '$lib/components/ui/tabs';
</script>

<Tabs.Root value="conversation">
  <Tabs.List>
    <Tabs.Trigger value="conversation">Conversation</Tabs.Trigger>
    <Tabs.Trigger value="verify">Verify</Tabs.Trigger>
  </Tabs.List>
  <Tabs.Content value="conversation">Session conversation</Tabs.Content>
  <Tabs.Content value="verify">Verification evidence</Tabs.Content>
</Tabs.Root>
