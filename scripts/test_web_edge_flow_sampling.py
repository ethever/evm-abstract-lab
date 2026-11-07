#!/usr/bin/env python3
"""Exercise edge-flow sampling without a browser or an application clock."""

from copy import deepcopy
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import web_test_edge_flow as edge_flow

from web_test_edge_flow import assert_flow_direction
from web_test_edge_flow_sampling import capture_interval, sampling_readiness


REGION = [0, 0, 100, 100]
MAX_INTERVAL = 56 / (2 * 60)


def frame(index, y, *, step=0.04, bounds=None, x=50):
    markers = [] if y is None else [{
        "center": [x, y], "pixels": 16,
        "bounds": bounds or [x - 2, y - 2, x + 2, y + 2],
    }]
    return {"capture_timestamp_seconds": 100 + index * step,
            "animation_seconds": index * step,
            "regions": {"edge": {"bounds": REGION, "markers": markers}}}


class EdgeFlowSamplingTests(unittest.TestCase):
    def readiness(self, frames):
        return sampling_readiness(frames, ("edge",), MAX_INTERVAL)

    def test_stationary_markers_never_establish_observable_motion(self):
        frames = [frame(index, 50) for index in range(10)]
        state = self.readiness(frames)
        self.assertEqual(len(state["timing_pairs"]), 9)
        self.assertFalse(state["ready"])
        self.assertEqual(state["motion_pairs"], {"edge": []})
        with self.assertRaises(AssertionError):
            assert_flow_direction(frames, "edge", [0, -1])

    def test_multiple_stationary_heads_match_their_own_zero_displacement(self):
        markers = [frame(0, y)["regions"]["edge"]["markers"][0] for y in [15, 21, 27, 33]]
        frames = [frame(index, None) for index in range(10)]
        for item in frames:
            item["regions"]["edge"]["markers"] = deepcopy(markers)
        original = deepcopy(frames)
        state = self.readiness(frames)
        self.assertEqual(len(state["timing_pairs"]), 9)
        self.assertFalse(state["ready"])
        self.assertEqual(state["motion_pairs"], {"edge": []})
        self.assertEqual(frames, original)

    def test_reverse_motion_is_ready_but_fails_expected_direction(self):
        frames = [frame(index, 30 + index * 2) for index in range(10)]
        original = deepcopy(frames)
        state = self.readiness(frames)
        self.assertTrue(state["ready"])
        self.assertEqual(len(state["motion_pairs"]["edge"]), 9)
        with self.assertRaises(AssertionError):
            assert_flow_direction(frames, "edge", [0, -1])
        self.assertEqual(frames, original)

    def test_forward_motion_passes_the_independent_direction_oracle(self):
        frames = [frame(index, 70 - index * 2) for index in range(10)]
        self.assertTrue(self.readiness(frames)["ready"])
        result = assert_flow_direction(frames, "edge", [0, -1])
        self.assertEqual(len(result["advanced_pairs"]), 9)
        self.assertEqual(result["backward_steps"], [])

    def test_reverse_samples_are_retained_after_initial_forward_motion(self):
        frames = [frame(index, y) for index, y in enumerate([70, 68, 66, 68, 70, 72, 74, 76, 78, 80])]
        original = deepcopy(frames)
        self.assertTrue(self.readiness(frames)["ready"])
        with self.assertRaises(AssertionError):
            assert_flow_direction(frames, "edge", [0, -1])
        self.assertEqual(frames, original)

    def test_clipped_heads_do_not_finish_collection_before_visible_pairs(self):
        # The isolated failure had good cadence but only two clipped heads:
        # 0.69px and 0.625px adjacent movement, with six empty frames between.
        frames = [frame(0, 1.833, bounds=[48, 0, 52, 4]),
                  frame(1, 1.143, bounds=[48, 0, 52, 3])]
        frames.extend(frame(index, None) for index in range(2, 8))
        frames.extend([frame(8, 98, bounds=[48, 96, 52, 100]),
                       frame(9, 97.375, bounds=[48, 94, 52, 100])])
        state = self.readiness(frames)
        self.assertEqual(len(state["timing_pairs"]), 9)
        self.assertFalse(state["ready"])
        frames.extend(frame(index, y) for index, y in enumerate([94, 92, 90, 88], 10))
        self.assertTrue(self.readiness(frames)["ready"])
        assert_flow_direction(frames, "edge", [0, -1])

    def test_displaced_but_clipped_markers_cannot_establish_readiness(self):
        frames = [frame(index, 70 - index * 2, x=1.5, bounds=[0, 60 - index * 2, 3, 72 - index * 2])
                  for index in range(10)]
        self.assertFalse(self.readiness(frames)["ready"])

    def test_slow_cadence_and_exact_displacement_boundary_stay_rejected(self):
        slow = [frame(index, 70 - index * 2, step=0.5) for index in range(10)]
        self.assertFalse(self.readiness(slow)["ready"])
        self.assertEqual(self.readiness(slow)["timing_pairs"], [])
        boundary = [frame(index, 70 - index * 1.5) for index in range(10)]
        self.assertFalse(self.readiness(boundary)["ready"])

    def test_only_selected_active_regions_gate_collection_but_all_are_retained(self):
        frames = [frame(index, 70 - index * 2) for index in range(10)]
        for item in frames:
            item["regions"]["old"] = frame(0, 50)["regions"]["edge"]
        original = deepcopy(frames)
        self.assertTrue(sampling_readiness(frames, ("edge",), MAX_INTERVAL)["ready"])
        self.assertFalse(sampling_readiness(frames, ("edge", "old"), MAX_INTERVAL)["ready"])
        self.assertEqual(frames, original)

    def test_capture_uses_full_budget_and_decodes_only_after_screencast_stops(self):
        clock = SimpleNamespace(now=0.0)
        events = {}
        state = {"stopped": False, "captured": 0, "decoded": []}

        def send(method, _parameters=None):
            if method == "Page.stopScreencast":
                state["stopped"] = True

        cdp = SimpleNamespace(on=lambda name, callback: events.update({name: callback}),
                              send=send, detach=lambda: None)

        def wait(_milliseconds):
            index = state["captured"]
            clock.now += 0.08
            state["captured"] += 1
            events["Page.screencastFrame"]({
                "metadata": {"timestamp": 100 + index * 0.08}, "sessionId": index,
                "data": edge_flow.base64.b64encode(f"capture-{index}".encode()).decode(),
            })

        def decode(_page, png, _regions, _box):
            self.assertTrue(state["stopped"], "pixel decoding must not disturb capture cadence")
            index = int(png.decode().removeprefix("capture-"))
            state["decoded"].append(index)
            return frame(index, 70 - index * 2)["regions"]

        page = SimpleNamespace(viewport_size={"width": 1440, "height": 1000},
                               context=SimpleNamespace(new_cdp_session=lambda _page: cdp),
                               wait_for_timeout=wait)
        with TemporaryDirectory() as temporary, patch.object(edge_flow.time, "monotonic", lambda: clock.now), \
                patch.object(edge_flow, "flow_pixels", side_effect=decode):
            output = Path(temporary)
            frames = edge_flow.flow_frames(page, Mock(), output, "capture", {"edge": REGION})
            self.assertEqual(len(frames), 24)
            self.assertEqual(state["decoded"], list(range(24)))
            self.assertEqual(len(list(output.glob("capture-[0-9][0-9].png"))), 24)
            self.assertTrue((output / "capture-capture-metadata.json").is_file())
            self.assertTrue((output / "capture-frames.json").is_file())
            self.assertTrue((output / "capture-sampling.json").is_file())
            assert_flow_direction(frames, "edge", [0, -1])

    def test_pacing_preserves_period_coverage_inside_original_capture_budget(self):
        period = 56 / 60
        # These cover the original 44px chain gap and the >25px return gap.
        # Even two immediately delivered startup frames leave enough paced
        # samples for a complete original head period inside the 24-frame cap.
        for height in [25, 42, 44, 56]:
            limit = min(56, height) / (2 * 60)
            interval = capture_interval(limit)
            self.assertGreaterEqual((24 - 2) * interval, period)
            self.assertLess(interval, limit)


if __name__ == "__main__":
    unittest.main()
