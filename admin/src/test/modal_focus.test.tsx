import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Modal } from "@/components/ui/dialog";

const nextFrame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));

function Dialog({ onClose }: { onClose: () => void }) {
  return (
    <Modal open onClose={onClose} title="Pick someone">
      <label htmlFor="who">Who</label>
      <input id="who" />
    </Modal>
  );
}

describe("Modal focus", () => {
  it("does not take focus from a field the person is already typing in", async () => {
    render(<Dialog onClose={() => {}} />);
    const input = screen.getByLabelText("Who");
    input.focus();
    await nextFrame();
    await nextFrame();
    expect(document.activeElement).toBe(input);
  });

  it("keeps focus where it is when the parent re-renders with a new onClose", async () => {
    const { rerender } = render(<Dialog onClose={() => {}} />);
    await nextFrame();
    const input = screen.getByLabelText("Who");
    input.focus();
    rerender(<Dialog onClose={() => {}} />);
    await nextFrame();
    await nextFrame();
    expect(document.activeElement).toBe(input);
  });
});
