import * as React from "react"
import { cn } from "@/lib/utils"
import { ChevronDownIcon } from "lucide-react"

/**
 * A styled native `<select>`: keyboard and screen-reader behavior come from
 * the browser. `className` sizes the wrapper, which the select fills.
 */
function NativeSelect({ className, ...props }: React.ComponentProps<"select">) {
  return (
    <div className={cn("relative w-full", className)}>
      <select
        data-slot="native-select"
        className="h-[34px] w-full min-w-0 appearance-none rounded-md bg-raised shadow-[inset_0_0_0_1px_transparent] hover:shadow-[inset_0_0_0_1px_var(--input)] focus-visible:shadow-[inset_0_0_0_1px_var(--foreground)] py-1 pr-8 pl-3 text-base transition-[color,box-shadow] outline-none focus-visible:ring-[3px] focus-visible:ring-ring/35 disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 md:text-sm [&>option]:bg-popover [&>option]:text-popover-foreground"
        {...props}
      />
      <ChevronDownIcon className="pointer-events-none absolute top-1/2 right-2.5 size-4 -translate-y-1/2 opacity-50" />
    </div>
  )
}

export { NativeSelect }
