<template>
  <label :class="labelClass">
    <slot />
  </label>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { cn } from '@/lib/utils'

interface Props {
  class?: string
}

const props = defineProps<Props>()

const labelClass = computed(() =>
  cn(
    'text-[11px] font-semibold tracking-normal break-words text-muted-foreground peer-disabled:cursor-not-allowed peer-disabled:opacity-70',
    props.class
  )
)
</script>
