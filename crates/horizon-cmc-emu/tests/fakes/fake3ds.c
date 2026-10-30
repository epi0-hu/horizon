/* Azahar-like core: needs the full path (no ROM data), legacy option list, 400x480 XRGB8888 frames with the
 * 320x240 touch screen centred in the lower half, Circle Pad / C-stick / ZL / ZR, no SAVE_RAM, and a logging
 * thread of its own that hands the log callback heap strings and frees them right after the call. */
#include <stdio.h>
#include <stdint.h>
#include <stdbool.h>
#include <string.h>
#include <stdlib.h>
#include <pthread.h>
#include <unistd.h>
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
static uint32_t frame_px[400*480]; static int16_t audio[546*2]; static unsigned frame;
static pthread_t log_thread; static volatile int log_stop; static volatile int logged;
static int touch_x = -1, touch_y = -1, max_lx, min_ly, max_rx; static unsigned seen_mask;
/* TEST ONLY (real Azahar has no save memory): [0] path-only load ok, [1,2] touch, [3] buttons>>8, [4] circle x>0, [5] c-stick x>>8, [6] circle y<0 */
static uint8_t sram[16];

/* Like Azahar's "citra:Log" thread: format into a fresh heap string, log it, free it and scribble over it. */
static void* log_main(void* arg) {
  (void)arg;
  for (int i = 0; !log_stop; i++) {
    char* msg = malloc(400);
    int n = snprintf(msg, 400, "[ 12.%06d] Core.ARM11 <Info> thread log %d %s\n", i, i, i % 7 == 0 ? "with a long tail of text to cross the copy limit ......................................................................................................................................................................................................" : "");
    (void)n;
    logf_(i % 50 == 0 ? 2 : 1, msg);
    memset(msg, 'X', 399); msg[399] = 0;
    free(msg);
    __sync_fetch_and_add(&logged, 1);
    if (i % 64 == 0) usleep(200);
  }
  return NULL;
}

unsigned retro_api_version(void){ return 1; }
void retro_set_environment(env_t cb){ env=cb;
  unsigned ver=99; env(52,&ver); /* GET_CORE_OPTIONS_VERSION */
  struct retro_variable vars[]={
    {"citra_graphics_api","Graphics API; Auto|Vulkan|OpenGL|Software"},
    {"citra_layout_option","Screen Layout; default|single_screen|large_screen|side_by_side"},
    {"citra_swap_screen","Prominent 3DS Screen; Top|Bottom"},
    {"citra_analog_function","Right Analog Function; c_stick_and_touchscreen|touchscreen_pointer|c_stick"},
    {"citra_use_libretro_save_path","Save Data Location; LibRetro Default|Azahar Default"},
    {"citra_input_type","Microphone Input Type; auto|none|static_noise|frontend"},
    {NULL,NULL}};
  bool ok=env(16,vars);
  printf("[3ds] options version=%u set_variables=%d\n", ver, ok); }
void retro_set_video_refresh(video_t v){ video=v; } void retro_set_audio_sample(void* a){} void retro_set_audio_sample_batch(abatch_t a){ abatch=a; }
void retro_set_input_poll(poll_t p){ ipoll=p; } void retro_set_input_state(istate_t s){ istate=s; }
void retro_set_controller_port_device(unsigned p, unsigned d){}
void retro_init(void){
  struct retro_log_callback l={0}; if (env(27,&l)) logf_=l.log;
  log_stop=0; pthread_create(&log_thread, NULL, log_main, NULL); }
void retro_deinit(void){ log_stop=1; pthread_join(log_thread, NULL); printf("[3ds] deinit, log thread wrote %d lines\n", logged); }
void retro_get_system_info(struct retro_system_info* i){ i->library_name="fake Azahar"; i->library_version="2125"; i->valid_extensions="3ds|3dsx|cci|cxi"; i->need_fullpath=true; i->block_extract=false; }
void retro_get_system_av_info(struct retro_system_av_info* a){ a->geometry=(struct retro_game_geometry){400,480,7200,4800,400.0f/480}; a->timing=(struct retro_system_timing){60.0,32728}; }
static const char* get(const char* k){ struct retro_variable v={k,NULL}; return env(15,&v)&&v.value?v.value:"(none)"; }
bool retro_load_game(const struct retro_game_info* g){
  printf("[3ds] options api=%s layout=%s swap=%s analog=%s saves=%s mic=%s\n", get("citra_graphics_api"), get("citra_layout_option"),
    get("citra_swap_screen"), get("citra_analog_function"), get("citra_use_libretro_save_path"), get("citra_input_type"));
  if (strcmp(get("citra_graphics_api"), "Software") != 0) return false; /* we can't take a GPU context */
  FILE* f = fopen(g->path, "rb"); char magic[5]={0};
  if (f) { fseek(f, 0x100, SEEK_SET); if (fread(magic,1,4,f)!=4) magic[0]=0; fclose(f); }
  printf("[3ds] load path=%s data=%p size=%zu magic@0x100=%s\n", g->path, g->data, g->size, magic);
  unsigned fmt=1; if(!env(10,&fmt)) return false;
  struct retro_system_av_info av; retro_get_system_av_info(&av); env(37,&av.geometry); /* SET_GEOMETRY before av info is known */
  sram[0] = (g->data==NULL && g->size==0);
  return g->data==NULL && g->size==0; }
void retro_run(void){ ipoll();
  /* Same maths as Azahar's MouseTracker: pointer -> frame pixel, accepted on the touch screen (40..360, 240..480). */
  if (istate(0,6,0,2)) {
    int px=istate(0,6,0,0), py=istate(0,6,0,1);
    int x=(int)((px+0x7fff)/(float)(0x7fff*2)*400), y=(int)((py+0x7fff)/(float)(0x7fff*2)*480);
    if (x>=40 && x<360 && y>=240 && y<480) { touch_x=x-40; touch_y=y-240; }
  }
  for (unsigned id=0; id<16; id++) if (istate(0,1,0,id)) seen_mask |= 1u<<id;
  int lx=istate(0,5,0,0), ly=istate(0,5,0,1), rx=istate(0,5,1,0);
  if (lx>max_lx) max_lx=lx; if (ly<min_ly) min_ly=ly; if (rx>max_rx) max_rx=rx;
  for (int y=0;y<480;y++) for (int x=0;x<400;x++)
    frame_px[y*400+x] = y<240 ? 0x00FF0000 : (x>=40 && x<360 ? 0x0000FF00 : 0);
  video(frame_px,400,480,400*4);
  abatch(audio,546);
  sram[1]=(uint8_t)touch_x; sram[2]=(uint8_t)touch_y; sram[3]=(uint8_t)(seen_mask>>8); sram[4]=max_lx>0; sram[5]=(uint8_t)(max_rx>>8); sram[6]=min_ly<0;
  frame++; }
void retro_reset(void){} void retro_unload_game(void){ printf("[3ds] unload frames=%u touch=(%d,%d) buttons=0x%x circle x=%d y=%d cstick x=%d\n", frame, touch_x, touch_y, seen_mask, max_lx, min_ly, max_rx); }
unsigned retro_get_region(void){ return 0; }
size_t retro_get_memory_size(unsigned id){ return id==0 ? sizeof sram : 0; } void* retro_get_memory_data(unsigned id){ return id==0 ? sram : NULL; }
size_t retro_serialize_size(void){return 0;} bool retro_serialize(void*d,size_t s){return false;} bool retro_unserialize(const void*d,size_t s){return false;}
void retro_cheat_reset(void){} void retro_cheat_set(unsigned i,bool e,const char*c){} bool retro_load_game_special(unsigned t,const void*i,size_t n){return false;}
