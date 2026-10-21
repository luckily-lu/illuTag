import {
  computed,
  shallowRef,
  toValue,
  watch,
  type MaybeRefOrGetter,
} from "vue";

// A polling request may return the same failure after the user has closed it.
// A resolved error can alert again on a later attempt; a transient retry clear
// must not turn a dismissed polling failure into a new occurrence.
export function useErrorDialog(
  errors: MaybeRefOrGetter<readonly string[]>,
  pending: MaybeRefOrGetter<boolean> = false,
) {
  const dismissed = new Set<string>();
  const messages = shallowRef<string[]>([]);
  watch(
    () =>
      [
        [
          ...new Set(
            toValue(errors)
              .map((value) => value.trim())
              .filter(Boolean),
          ),
        ],
        toValue(pending),
      ] as const,
    ([values, waiting]) => {
      if (!waiting)
        for (const message of dismissed)
          if (!values.includes(message)) dismissed.delete(message);
      const next = values.filter((value) => !dismissed.has(value));
      if (next.length || !waiting) messages.value = next;
    },
    { immediate: true },
  );
  function close() {
    for (const message of messages.value) dismissed.add(message);
    messages.value = [];
  }
  return { messages, open: computed(() => messages.value.length > 0), close };
}
