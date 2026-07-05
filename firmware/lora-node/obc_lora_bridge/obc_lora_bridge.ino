// obc_lora_bridge — OBC LoRa-mesh node firmware
// ================================================
// A transparent USB-serial <-> LoRa-radio bridge. This node is a *dumb modem*:
// it relays opaque bytes in both directions and knows nothing about their meaning.
// All mesh semantics (heartbeats, assignments, the fleet auction) live on the OBC
// host, which talks to this node through `src/spine/lora_mesh.rs::SerialMeshRadio`.
//
// Wire protocol (must match the host codec in src/spine/lora_mesh.rs):
//   * Host -> node : one newline-terminated line per frame on USB serial.
//                    Each line is a compact JSON MeshFrame, e.g.
//                    {"t":"hb","n":"rover-a","x":1.0,"y":2.0,"b":0.9,"m":"explore"}
//                    The node transmits the line's bytes (minus the '\n') over LoRa.
//   * Node -> host : for each LoRa packet received, the node writes the payload
//                    bytes followed by '\n' to USB serial. The host's RX loop
//                    (`run_serial_rx`) decodes each line and bridges it into the
//                    fleet Coordinator.
//
// The node never parses the JSON — it only moves bytes. That keeps the firmware
// tiny and lets the frame format evolve host-side without reflashing radios.
//
// Radio library: RadioLib 6.x (https://github.com/jgromes/RadioLib). Install via
// the Arduino Library Manager ("RadioLib" by Jan Gromes). RadioLib abstracts the
// SX127x (T-Beam / Heltec V2) and SX126x (RAK4631 / newer T-Beam Supreme) chips
// behind one API, so this single sketch covers all four target boards — pick your
// board with the #define below.
//
// STATUS: reference firmware. This has NOT been compiled or flashed by its author;
// it mirrors RadioLib's documented 6.x API and OBC's serial framing. Treat pin maps
// and radio params as a starting point — verify against your board's schematic and
// your regulatory region before transmitting.

#include <RadioLib.h>
#include <SPI.h>

// ─── Board selection ────────────────────────────────────────────────────────────
// Uncomment exactly one. Pin maps below are the common community values; confirm
// against your specific board revision.
#define BOARD_TBEAM_SX1276      // LilyGO T-Beam v1.x (SX1276, 433/868/915)
// #define BOARD_HELTEC_V2_SX1276  // Heltec WiFi LoRa 32 v2 (SX1276)
// #define BOARD_HELTEC_V3_SX1262  // Heltec WiFi LoRa 32 v3 (SX1262, ESP32-S3)
// #define BOARD_RAK4631_SX1262    // RAK4631 / WisBlock (SX1262)
// #define BOARD_TDECK_SX1262      // LilyGO T-Deck / T-Deck Plus (SX1262, ESP32-S3)

// ─── Radio parameters ───────────────────────────────────────────────────────────
// FREQUENCY MUST MATCH your host LoraMeshConfig.freq_mhz AND your region's rules.
// US ISM = 915.0 MHz; EU868 = 868.0 MHz. Bandwidth/SF/CR are a long-range default
// (matches typical Meshtastic "LongFast"-ish reach); they must be identical on
// every node in the mesh or they will not hear each other.
static const float   RADIO_FREQ_MHZ = 915.0;   // <-- set to your region
static const float   RADIO_BW_KHZ   = 125.0;   // bandwidth
static const uint8_t RADIO_SF        = 10;      // spreading factor (7..12)
static const uint8_t RADIO_CR        = 5;       // coding rate 4/5..4/8 -> 5..8
static const uint8_t RADIO_SYNCWORD  = 0x2B;    // private mesh sync word
static const int8_t  RADIO_POWER_DBM = 17;      // TX power (respect regional EIRP)
static const uint16_t RADIO_PREAMBLE = 8;

// Hard cap on a single frame's payload. Must be <= host LoraMeshConfig.max_payload
// (230). Lines longer than this are dropped rather than truncated on-air.
static const size_t MAX_FRAME = 230;

static const unsigned long SERIAL_BAUD = 115200;  // must match SerialMeshRadio baud

// Bench bring-up: set to 1 to periodically self-transmit a heartbeat frame over
// LoRa — no host needed. Flash two boards with this on and each should print the
// other's heartbeat line on its serial monitor, proving the radio link + framing.
// Leave at 0 for normal operation.
#define SELFTEST_HEARTBEAT 0
static const unsigned long SELFTEST_PERIOD_MS = 5000;

// ─── Board pin maps + radio instance ────────────────────────────────────────────
#if defined(BOARD_TBEAM_SX1276)
  // T-Beam v1.x: SX1276 on the SPI bus.  NSS=18 DIO0=26 RST=23 DIO1=33
  SX1276 radio = new Module(18, 26, 23, 33);
#elif defined(BOARD_HELTEC_V2_SX1276)
  // Heltec WiFi LoRa 32 v2: NSS=18 DIO0=26 RST=14 DIO1=35
  SX1276 radio = new Module(18, 26, 14, 35);
#elif defined(BOARD_HELTEC_V3_SX1262)
  // Heltec WiFi LoRa 32 v3 (ESP32-S3): SX1262 on a *dedicated* SPI bus.
  // NSS=8 DIO1=14 RST=12 BUSY=13 ; SPI SCK=9 MISO=11 MOSI=10
  SX1262 radio = new Module(8, 14, 12, 13);
  // Non-default SPI pins — must be applied with SPI.begin() before radio.begin().
  #define LORA_SPI_SCK  9
  #define LORA_SPI_MISO 11
  #define LORA_SPI_MOSI 10
  #define LORA_SPI_NSS  8
#elif defined(BOARD_RAK4631_SX1262)
  // RAK4631 WisBlock: NSS=42 DIO1=47 RST=38 BUSY=46
  SX1262 radio = new Module(42, 47, 38, 46);
#elif defined(BOARD_TDECK_SX1262)
  // LilyGO T-Deck / T-Deck Plus (ESP32-S3): SX1262 on the board's *shared* SPI bus
  // (display + SD card live there too — fine for this dumb-modem sketch, which
  // never touches them). NSS=9 DIO1=45 RST=17 BUSY=13 ; SPI SCK=40 MISO=38 MOSI=41.
  // Pin map verified against Xinyuan-LilyGO/T-Deck examples/UnitTest/utilities.h.
  SX1262 radio = new Module(9, 45, 17, 13);
  #define LORA_SPI_SCK  40
  #define LORA_SPI_MISO 38
  #define LORA_SPI_MOSI 41
  #define LORA_SPI_NSS  9
  // T-Deck gotcha: every peripheral (radio included) is behind a power-gate pin.
  // BOARD_POWERON (GPIO10) must be driven HIGH before radio.begin() will talk.
  #define TDECK_POWERON 10
#else
  #error "Select a board: define one of BOARD_TBEAM_SX1276 / BOARD_HELTEC_V2_SX1276 / BOARD_HELTEC_V3_SX1262 / BOARD_RAK4631_SX1262 / BOARD_TDECK_SX1262"
#endif

// ─── RX interrupt plumbing ──────────────────────────────────────────────────────
// RadioLib fires this ISR when a packet lands. We only set a flag; the actual read
// happens in loop() at task level (reading SPI inside an ISR is unsafe).
volatile bool packetReady = false;

#if defined(ESP32) || defined(ESP8266)
  ICACHE_RAM_ATTR
#endif
void onPacketReceived() {
  packetReady = true;
}

// ─── Host -> node line assembly ─────────────────────────────────────────────────
static char   lineBuf[MAX_FRAME + 1];
static size_t lineLen = 0;
static bool   lineOverflow = false;

void setup() {
  Serial.begin(SERIAL_BAUD);
  while (!Serial && millis() < 3000) { /* wait for USB CDC on native-USB boards */ }

  // T-Deck: release the board-wide peripheral power gate before touching the radio.
#ifdef TDECK_POWERON
  pinMode(TDECK_POWERON, OUTPUT);
  digitalWrite(TDECK_POWERON, HIGH);
  delay(100);  // let the gated rail settle before SPI traffic
#endif

  // Boards that put the LoRa radio on a non-default SPI bus (e.g. Heltec V3) must
  // configure those pins before RadioLib touches the chip.
#ifdef LORA_SPI_SCK
  SPI.begin(LORA_SPI_SCK, LORA_SPI_MISO, LORA_SPI_MOSI, LORA_SPI_NSS);
#endif

  int state = radio.begin(RADIO_FREQ_MHZ, RADIO_BW_KHZ, RADIO_SF, RADIO_CR,
                          RADIO_SYNCWORD, RADIO_POWER_DBM, RADIO_PREAMBLE);
  if (state != RADIOLIB_ERR_NONE) {
    // Report the failure on serial and halt — a mis-inited radio is worse than none.
    Serial.print(F("{\"t\":\"err\",\"n\":\"lora-node\",\"m\":\"radio_init "));
    Serial.print(state);
    Serial.println(F("\"}"));
    while (true) { delay(1000); }
  }

  radio.setPacketReceivedAction(onPacketReceived);  // RadioLib 6.x API
  radio.startReceive();
}

void loop() {
  pumpSerialToRadio();
  pumpRadioToSerial();
  maybeSelfTest();
}

// Bench self-test: periodically originate a heartbeat over LoRa so two boards can
// be validated end-to-end without the OBC host. Compiled out unless enabled.
void maybeSelfTest() {
#if SELFTEST_HEARTBEAT
  static unsigned long last = 0;
  unsigned long nowMs = millis();
  if (nowMs - last >= SELFTEST_PERIOD_MS) {
    last = nowMs;
    static const char hb[] = "{\"t\":\"hb\",\"n\":\"selftest\",\"m\":\"idle\"}";
    transmitFrame((const uint8_t*)hb, sizeof(hb) - 1);
    Serial.println(F("[selftest] heartbeat transmitted"));
  }
#endif
}

// Drain USB serial; on each complete line, transmit its bytes over LoRa.
void pumpSerialToRadio() {
  while (Serial.available() > 0) {
    char c = (char)Serial.read();
    if (c == '\n' || c == '\r') {
      if (lineLen > 0 && !lineOverflow) {
        transmitFrame((const uint8_t*)lineBuf, lineLen);
      }
      lineLen = 0;
      lineOverflow = false;
    } else if (lineLen < MAX_FRAME) {
      lineBuf[lineLen++] = c;
    } else {
      // Line exceeds a single LoRa frame — drop it rather than fragment on-air.
      lineOverflow = true;
    }
  }
}

// Half-duplex TX: stop RX, send, resume RX. transmit() is blocking, which is fine
// for the low frame rate a fleet heartbeat/assignment stream produces.
void transmitFrame(const uint8_t* data, size_t len) {
  int state = radio.transmit((uint8_t*)data, len);
  if (state != RADIOLIB_ERR_NONE) {
    Serial.print(F("{\"t\":\"err\",\"n\":\"lora-node\",\"m\":\"tx "));
    Serial.print(state);
    Serial.println(F("\"}"));
  }
  radio.startReceive();  // return to listening after any TX
}

// On the ISR flag, read the packet and emit it to the host as one newline-framed
// line. readData null-terminates the buffer, so we can write it as a C string.
void pumpRadioToSerial() {
  if (!packetReady) return;
  packetReady = false;

  uint8_t buf[MAX_FRAME + 1];
  int len = radio.getPacketLength();
  if (len <= 0 || (size_t)len > MAX_FRAME) {
    radio.startReceive();
    return;
  }
  int state = radio.readData(buf, len);
  if (state == RADIOLIB_ERR_NONE) {
    Serial.write(buf, len);
    Serial.write('\n');
  }
  radio.startReceive();  // re-arm for the next packet
}
