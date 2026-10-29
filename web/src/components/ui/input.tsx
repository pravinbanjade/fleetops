import * as React from "react"
import { cn } from "@/lib/utils"

function Input({ className, type, ...props }: React.ComponentProps<"input">) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(
        "h-[34px] w-full min-w-0 rounded-md bg-raised px-3 py-1 text-base shadow-[inset_0_0_0_1px_transparent] transition-[color,box-shadow] outline-none file:inline-flex file:h-7 file:border-0 file:bg-transparent file:text-sm file:font-medium file:text-foreground placeholder:text-meta hover:shadow-[inset_0_0_0_1px_var(--input)] disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 md:text-sm",
        "focus-visible:shadow-[inset_0_0_0_1px_var(--foreground)] focus-visible:ring-[3px] focus-visible:ring-ring/35",
        "aria-invalid:border-destructive aria-invalid:ring-destructive dark:aria-invalid:ring-destructive",
        className
      )}
      {...props}
    />
  )
}

export { Input }
