# Repository compatibility checks

Inspection covers the named historical controller projects and local active HDL
projects. It is not a claim that every fork on the account has been tested.

| Repository | Inspected revision | Test convention |
| --- | --- | --- |
| meawoppl/MagicSchoolBus | d96c531aa07af7938f5235d294040791f5cd3749 | endiatx.py; src/verilog_test/*_tb.v; all sibling RTL; Icarus assertions |
| coup-de-foudre/widlar | 0b0632bbc8295c03fea87e5de104ce085bb58460 | tool.py; tesla_ctl/fpga_test/*_tb.v; stable output directory; memory_tb.gtkw |
| coup-de-foudre/fpga-tesla | 8ba88a1437bd74822c2db651b391595ebb005d49 | dev_tool.py; src/verilog_test/led_tb.v; output.vcd |
| gps-time-calibrator (local) | working tree inspected | fpga/rtl_test; local Yosys cells_sim.v; inherited tool.py defaults require checking |
| affogato (local) | working tree inspected | fpga/rtl; Verilator lint in Makefile; no matching local testbench files found |
| meawoppl/nextpnr-bug-repro | default-branch script inspected | reproduce.sh runs Yosys synth_ice40 then nextpnr-ice40; no testbench |
| meawoppl/UPDuino_v2_0 | fork tree and README inspected | board/example repository; no matching test runner found |
| meawoppl/JPEG_Encoder | fork README and tb_je_ip.v inspected | Active-HDL simulation tree; tb_ prefix; reads external YUYV and ROM assets |

Validation on Icarus 12:

- MagicSchoolBus's full discovered suite fails compilation on duplicate UartRx
  and UartTx port declarations. The focused CameraClock source/bench pair passes
  and produces VCD. Explicit source configuration can isolate that pair without
  editing the design.
- Widlar's full discovered suite fails compilation on the MidiSequencer port
  comma and top.v syntax. The focused MaxDetector source/bench pair passes and
  produces VCD.
- fpga-tesla's LED bench compiles, exits zero, but prints its timeout ERROR.
  The plugin correctly classifies the result as failed and retains output.vcd.

These are observed source/tool compatibility results, not fixes to the controller
repositories. The plugin's counter example and isolated runner tests validate
the success path independently.

The JPEG fork is not covered by the Icarus autodiscovery adapter: its top needs
an explicit source list, its runtime assets need staging, and its file-open error
uses a plain display message rather than ERROR/$fatal. Do not label that bench
passing based solely on its process exit status. The synthesis reproducer can
use tool argument forwarding; synthesis is not presented as a simulation test.

GTKWave save files and decimal/hex translation-filter conventions in Widlar
motivate stable per-test browser view state. This implementation does not parse
the .gtkw flag format or emulate enum filters. See the main README for supported
waveform formats and explicit limits.
