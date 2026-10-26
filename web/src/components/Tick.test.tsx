import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Tick } from "./Tick";

describe("Tick", () => {
  it("lights only the changed digit in the rise colour when the value goes up", () => {
    const { container, rerender } = render(<Tick value={12.4} text="12.4%" />);
    rerender(<Tick value={12.7} text="12.7%" />);
    const lit = container.querySelectorAll(".tick-rise");
    expect(lit).toHaveLength(1);
    expect(lit[0]).toHaveTextContent("7");
    expect(container.querySelectorAll(".tick-fall")).toHaveLength(0);
  });

  it("uses the fall colour when the value goes down", () => {
    const { container, rerender } = render(<Tick value={12.7} text="12.7%" />);
    rerender(<Tick value={12.4} text="12.4%" />);
    expect(container.querySelectorAll(".tick-fall")).toHaveLength(1);
  });

  it("lights nothing when the text does not change", () => {
    const { container, rerender } = render(<Tick value={5} text="5" />);
    rerender(<Tick value={5} text="5" />);
    expect(container.querySelectorAll(".tick-rise, .tick-fall")).toHaveLength(0);
  });

  it("never paints a background behind the digits", () => {
    const { container, rerender } = render(<Tick value={1} text="1.0%" />);
    rerender(<Tick value={2} text="2.0%" />);
    for (const el of container.querySelectorAll<HTMLElement>("*")) expect(el.style.background + el.style.backgroundColor).toBe("");
  });

  it("reads as the whole value", () => {
    const { container } = render(<Tick value={3} text="3.1%" />);
    expect(container).toHaveTextContent("3.1%");
  });
});
