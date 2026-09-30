import * as React from "react"
import { cn } from "@/lib/utils"

function Input({ className, type, ...props }: React.ComponentProps<"input">) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(
        "h-9 w-full min-w-0 rounded-md border border-input bg-card px-3 py-1 text-base shadow-[inset_0_1px_1px_oklch(0_0_0/0.05)] transition-[color,box-shadow] outline-none file:inline-flex file:h-7 file:border-0 file:bg-card file:text-sm file:font-medium file:text-foreground placeholder:text-muted-foreground disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 md:text-sm dark:bg-background/60",
        "focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/35",
        "aria-invalid:border-destructive aria-invalid:ring-destructive dark:aria-invalid:ring-destructive",
        className
      )}
      {...props}
    />
  )
}

export { Input }
