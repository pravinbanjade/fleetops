import * as React from "react"
import { cn } from "@/lib/utils"

function Textarea({ className, ...props }: React.ComponentProps<"textarea">) {
  return (
    <textarea
      data-slot="textarea"
      className={cn(
        "flex field-sizing-content min-h-16 w-full rounded-md bg-raised shadow-[inset_0_0_0_1px_transparent] hover:shadow-[inset_0_0_0_1px_var(--input)] focus-visible:shadow-[inset_0_0_0_1px_var(--foreground)] px-3 py-2 text-base transition-[color,box-shadow] outline-none placeholder:text-meta focus-visible:ring-[3px] focus-visible:ring-ring/35 disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-destructive md:text-sm",
        className
      )}
      {...props}
    />
  )
}

export { Textarea }
