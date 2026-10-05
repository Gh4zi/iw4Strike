# Client audio

Bevy supplies cue intentions, listener snapshots and desired source scenes.
Control selects cues, waits for media, admits layers and updates spatial gain.
CueFeedback observes decisions; persistent sources wait for media in control.

```text
presentation / immutable bank requests
                  ↓
       bounded cue intake queue
                  ↓
      AudioControl (audio-control thread)
       choices / instances / assignments
                  ↓
      AudioRender (render_core.rs)
                  ↓
       CPAL device / null / offline
```

The internal rate is 48 kHz, the quantum 128 stereo frames and the physical
pool 128 slots. Control stores 2048 logical instances and processes at most
64 cue intakes and 64 pending cue steps per pass. Pools refuse work;
physical saturation keeps loops virtual and rejects new one-shots. Cancellation
and epoch invalidation bypass queues; 4096 versioned cue keys preserve Stop/Play.

Cue slots/selectors cap at 2048; media caps 4096 keys/256 jobs, prefetch submits 64/pass.
Entity occurrences and animation marker instances retain distinct identities.
Control dedups 8192 identities within 100 ticks; local life invalidates marker cues.
Loaded keys use content/conversion identity; PCM pins 256 MiB; Symphonia/ADPCM scratch reserves 64 MiB.
Control checks all rules and physical reservations before stopping victims.
Cues retain media/deadlines; diagnostics separate pending source layers and voices.

Loops publish desired cue scenes keyed by scope/epoch/object/slot and version.
Sources use model/map/menu/local-life slots and versions, retain virtual cursors and stop
on desired absence. Control ranks eight map voices with a 0.002 gain floor.
Shellshock and heartbeat follow client/life state. Scenes cap at 2048 execution
keys and 4096 assertions/version records; exclusion preserves virtual cues.
Removed versions cannot resurrect; excess keys/refused updates are counted.

The callback reads prepared PCM/slots, resamples and sums fixed blocks. Control
frees retired payloads; media joins run on retirement threads. Stereo gain/pan is atomic;
listener snapshots stay in control; master volume is applied once after mixing.

`audio-device` owns streams/recovery and counts recoverable underruns. Control
uses null while output is unavailable. Virtualization hands the cursor back to
control. Source/device resampling uses linear interpolation. Match/channel ramps
and cue fades/releases use AudioFrame. Match gain preserves cues; mix tokens remain unfinished.
`OfflineRenderer` shares the kernel, with scheduled PCM, status and cursor.
Canonical action identities, cue groups, streaming, DSP buses/tails and
acoustic propagation remain unfinished, as do media budgets and time mappings.
