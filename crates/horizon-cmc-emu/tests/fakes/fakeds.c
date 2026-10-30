#include <stdio.h>
#include <stdint.h>
#include <stdbool.h>
#include <string.h>
#include <stdlib.h>
typedef bool (*env_t)(unsigned, void*);
typedef void (*video_t)(const void*, unsigned, unsigned, size_t);
typedef size_t (*abatch_t)(const int16_t*, size_t);
typedef void (*poll_t)(void);
typedef int16_t (*istate_t)(unsigned, unsigned, unsigned, unsigned);
typedef void (*log_t)(int, const char*, ...);
struct retro_log_callback { log_t log; };
struct retro_variable { const char* key; const char* value; };
struct retro_game_info { const char* path; const void* data; size_t size; const char* meta; };
struct retro_system_info { const char* library_name; const char* library_version; const char* valid_extensions; bool need_fullpath; bool block_extract; };
struct retro_game_geometry { unsigned base_width, base_height, max_width, max_height; float aspect_ratio; };
struct retro_system_timing { double fps, sample_rate; };
struct retro_system_av_info { struct retro_game_geometry geometry; struct retro_system_timing timing; };
static env_t env; static video_t video; static abatch_t abatch; static poll_t ipoll; static istate_t istate; static log_t logf_;
static uint8_t sram[512*1024]; static uint32_t frame_px[256*384]; static int16_t audio[548*2]; static unsigned frame;
static bool bitmasks;
unsigned retro_api_version(void){ return 1; }
void retro_set_environment(env_t cb){ env=cb;
  struct retro_log_callback l={0}; if (env(27,&l)) logf_=l.log;
  void* rumble[1]={0}; bool r=env(23,rumble);
  bitmasks = env(51|0x10000, NULL);
  unsigned mv=99; bool m=env(59,&mv);
  const char* sd=NULL; env(31,&sd); const char* sys=NULL; env(9,&sys);
  struct { unsigned v; void* i; } vfs={3,0}; bool v=env(45|0x10000,&vfs);
  void* ft[2]={0}; bool f=env(21,ft);
  printf("[ds] set_env log=%d rumble=%d(%p) bitmasks=%d msgver=%d/%u vfs=%d frametime=%d save=%s\n", logf_!=0, r, rumble[0], bitmasks, m, mv, v, f, sd?sd:"(null)"); }
void retro_set_video_refresh(video_t v){ video=v; } void retro_set_audio_sample(void* a){} void retro_set_audio_sample_batch(abatch_t a){ abatch=a; }
void retro_set_input_poll(poll_t p){ ipoll=p; } void retro_set_input_state(istate_t s){ istate=s; }
void retro_set_controller_port_device(unsigned p, unsigned d){ printf("[ds] port %u device %u\n",p,d); }
void retro_init(void){ if (logf_) logf_(1, "fake melonDS DS %s\n", "1.0"); }
void retro_deinit(void){ printf("[ds] deinit\n"); }
void retro_get_system_info(struct retro_system_info* i){ i->library_name="fake melonDS DS"; i->library_version="1.0"; i->valid_extensions="nds|ids|dsi"; i->need_fullpath=false; i->block_extract=false; }
void retro_get_system_av_info(struct retro_system_av_info* a){ a->geometry=(struct retro_game_geometry){256,384,256,385,256.0f/384}; a->timing=(struct retro_system_timing){59.8261,32768}; }
static const char* get(const char* k){ struct retro_variable v={k,NULL}; return env(15,&v)&&v.value?v.value:"(none)"; }
bool retro_load_game(const struct retro_game_info* g){
  unsigned fmt=1; if(!env(10,&fmt)) { printf("[ds] no XRGB8888\n"); return false; }
  struct retro_variable vars[]={
    {"melonds_render_mode","Render mode; opengl|software"},
    {"melonds_screen_layout1","Layout; left-right|top-bottom|bottom-top"},
    {"melonds_screen_gap","Gap; 8|0|1|2"},
    {"melonds_touch_mode","Touch; auto|joystick|touch"},
    {"melonds_jit_fast_memory","Fastmem; enabled|disabled"},
    {"melonds_mic_input","Mic; microphone|silence|noise|blow"},
    {NULL,NULL}};
  env(16,vars);
  printf("[ds] options render=%s layout=%s gap=%s touch=%s fastmem=%s mic=%s\n", get("melonds_render_mode"), get("melonds_screen_layout1"),
    get("melonds_screen_gap"), get("melonds_touch_mode"), get("melonds_jit_fast_memory"), get("melonds_mic_input"));
  const char* user=NULL; env(38,&user);
  uint64_t sum=0; const uint8_t* d=g->data; for (size_t i=0;i<g->size;i+=4096) sum+=d[i]; 
  ((uint8_t*)g->data)[0]^=0xFF; /* cores may scribble on the buffer: must not crash or touch the file */
  printf("[ds] load %s size=%zu sum=%llu user=%s\n", g->path, g->size, (unsigned long long)sum, user?user:"(null)");
  return g->data!=NULL && g->size>0; }
void retro_run(void){ ipoll();
  uint32_t mask = bitmasks ? (uint16_t)istate(0,1,0,256) : 0;
  int16_t pressed=istate(0,6,0,2), px=istate(0,6,0,0), py=istate(0,6,0,1), cnt=istate(0,6,0,3), bad=istate(0,6,1,0);
  if (frame%30==0) printf("[ds] frame %u mask=0x%x pointer pressed=%d x=%d y=%d count=%d idx1=%d\n", frame, mask, pressed, px, py, cnt, bad);
  if (pressed) { int tx=(px+0x7fff)*256/0xfffe, ty=(py+0x7fff)*384/0xfffe - 192; sram[1]=(uint8_t)tx; sram[2]=(uint8_t)ty; }
  for (int i=0;i<256*384;i++) frame_px[i]= (i/256<192) ? 0x00FF0000 : 0x0000FF00;
  video(frame_px,256,384,256*4);
  abatch(audio,548);
  sram[3]=(uint8_t)(mask>>8); sram[0]=(uint8_t)frame; frame++; }
void retro_reset(void){} void retro_unload_game(void){ printf("[ds] unload sram0=%d touch=(%d,%d)\n", sram[0], sram[1], sram[2]); }
unsigned retro_get_region(void){ return 0; }
size_t retro_get_memory_size(unsigned id){ return id==0? sizeof sram:0;} void* retro_get_memory_data(unsigned id){ return id==0? sram:NULL;}
size_t retro_serialize_size(void){return 0;} bool retro_serialize(void*d,size_t s){return false;} bool retro_unserialize(const void*d,size_t s){return false;}
void retro_cheat_reset(void){} void retro_cheat_set(unsigned i,bool e,const char*c){} bool retro_load_game_special(unsigned t,const void*i,size_t n){return false;}
