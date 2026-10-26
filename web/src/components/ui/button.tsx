import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { cn } from "@/lib/utils"
import { Slot } from "radix-ui"

const buttonVariants = cva(
  "inline-flex shrink-0 items-center justify-center gap-2 rounded-full text-sm font-semibold whitespace-nowrap press outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-destructive dark:aria-invalid:ring-destructive [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        // The primary action is inverted, a white pill. Disabled, it drops to the
        // secondary step so the label stays legible instead of fading to grey on grey.
        default:
          "bg-primary text-primary-foreground hover:bg-primary/88 hover:scale-[1.02] disabled:bg-secondary disabled:text-muted-foreground disabled:opacity-100",
        destructive:
          "text-destructive shadow-[inset_0_0_0_1px_var(--destructive)] hover:bg-destructive hover:text-black focus-visible:ring-destructive",
        outline:
          "text-foreground shadow-[inset_0_0_0_1px_var(--input)] hover:shadow-[inset_0_0_0_1px_var(--foreground)]",
        secondary:
          "bg-secondary text-secondary-foreground hover:bg-[#2e2e2e]",
        ghost:
          "font-medium text-muted-foreground hover:bg-accent hover:text-accent-foreground",
        link: "h-auto! px-0! font-medium text-foreground underline decoration-meta underline-offset-4 hover:decoration-foreground",
      },
      size: {
        default: "h-[34px] px-4 has-[>svg]:pl-3.5",
        xs: "h-6 gap-1 px-2.5 text-xs has-[>svg]:pl-2 [&_svg:not([class*='size-'])]:size-3",
        sm: "h-[30px] gap-1.5 px-3.5 has-[>svg]:pl-3",
        lg: "h-10 px-6 has-[>svg]:pl-5",
        icon: "size-[34px]",
        "icon-xs": "size-6 [&_svg:not([class*='size-'])]:size-3",
        "icon-sm": "size-[30px]",
        "icon-lg": "size-10",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  }
)

function Button({
  className,
  variant = "default",
  size = "default",
  asChild = false,
  ...props
}: React.ComponentProps<"button"> &
  VariantProps<typeof buttonVariants> & {
    asChild?: boolean
  }) {
  const Comp = asChild ? Slot.Root : "button"

  return (
    <Comp
      data-slot="button"
      data-variant={variant}
      data-size={size}
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  )
}

export { Button, buttonVariants }
