#!/usr/bin/env python3
"""Checks a device's ESPHome API (`esphome-api`) as Home Assistant uses it: handshake with the
key, device info, the entity list, the states, and a switch command answered by a new state.

Needs: `pip install aioesphomeapi`. The key from `api-key` on the device's console, in
ESPHOME_API_KEY (it is never printed).

    ESPHOME_API_KEY=... tools/hil/esphome.py iotkit-xxxx.local [--switch relay]
"""

import argparse
import asyncio
import os
import sys

from aioesphomeapi import APIClient, SwitchInfo


async def check(host: str, switch: str | None) -> None:
    client = APIClient(host, 6053, None, noise_psk=os.environ["ESPHOME_API_KEY"])
    await client.connect(login=True)
    info = await client.device_info()
    print(f"{info.name} ({info.model}), firmware {info.project_version}, MAC {info.mac_address}")
    entities, _ = await client.list_entities_services()
    for e in entities:
        print(f"  {type(e).__name__:28} {e.object_id}")
    states = {}
    changed = asyncio.Event()

    def on_state(state) -> None:
        states[state.key] = state
        changed.set()

    client.subscribe_states(on_state)
    await asyncio.sleep(2)
    print(f"{len(states)} states received")
    if switch:
        target = next(e for e in entities if isinstance(e, SwitchInfo) and e.object_id == switch)
        wanted = not getattr(states.get(target.key), "state", False)
        changed.clear()
        client.switch_command(target.key, wanted)
        async with asyncio.timeout(5):
            while getattr(states.get(target.key), "state", None) != wanted:
                await changed.wait()
                changed.clear()
        print(f"{switch} switched {'on' if wanted else 'off'}, state reported")
    await client.disconnect()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("host")
    parser.add_argument("--switch", help="a switch to toggle (and wait for its new state)")
    args = parser.parse_args()
    if "ESPHOME_API_KEY" not in os.environ:
        sys.exit("ESPHOME_API_KEY: the key `api-key` shows on the device's console")
    asyncio.run(check(args.host, args.switch))


if __name__ == "__main__":
    main()
