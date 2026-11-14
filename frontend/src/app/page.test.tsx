import { render, screen } from "@testing-library/react";
import type { ImgHTMLAttributes } from "react";
import { describe, expect, it, vi } from "vitest";

import Home from "./page";

// next/image needs build-time config that is absent under Vitest.
vi.mock("next/image", () => ({
  default: ({
    alt,
    ...props
  }: ImgHTMLAttributes<HTMLImageElement> & { priority?: boolean }) => {
    const { priority, ...rest } = props;
    void priority;
    // eslint-disable-next-line @next/next/no-img-element
    return <img alt={alt} {...rest} />;
  },
}));

describe("Home page", () => {
  it("renders the primary heading", () => {
    render(<Home />);
    expect(
      screen.getByRole("heading", { level: 1, name: /to get started/i }),
    ).toBeInTheDocument();
  });
});
