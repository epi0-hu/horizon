#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <ucontext.h>
typedef void (*log_t)(int, const char*, ...);
static log_t logcb; typedef bool (*cw_t)(unsigned, void*); static cw_t clearwaits; struct rumble { bool (*set)(unsigned, int, uint16_t); } rum;

struct retro_variable { const char *key; const char *value; };
struct retro_system_info { const char *library_name; const char *library_version; const char *valid_extensions; bool need_fullpath; bool block_extract; };
struct retro_game_geometry { unsigned base_width, base_height, max_width, max_height; float aspect_ratio; };
struct retro_system_timing { double fps, sample_rate; };
struct retro_system_av_info { struct retro_game_geometry geometry; struct retro_system_timing timing; };
struct retro_game_info { const char *path; const void *data; size_t size; const char *meta; };
typedef bool (*env_t)(unsigned, void*);
typedef void (*video_t)(const void*, unsigned, unsigned, size_t);
typedef void (*asample_t)(int16_t,int16_t);
typedef size_t (*abatch_t)(const int16_t*, size_t);
typedef void (*poll_t)(void);
typedef int16_t (*istate_t)(unsigned,unsigned,unsigned,unsigned);
static env_t env; static video_t video; static abatch_t abatch; static poll_t ipoll; static istate_t istate; static asample_t asample;
static uint32_t fb[320*240]; static int16_t audio[735*2]; static uint8_t sram[2048]; static int frame;
static ucontext_t main_ctx, game_ctx; static char *gstack; static int16_t gmask; static void game(void){ for(;;){ gmask = istate(0,1,0,256); abatch(audio,735); logcb(1, "coroutine frame mask=%d %s %llu\n", (int)gmask, "ok", 123456789012ULL); swapcontext(&game_ctx,&main_ctx);} }
unsigned retro_api_version(void){return 1;}
void retro_set_environment(env_t cb){ env=cb; unsigned v=99; bool ok=env(52,&v); printf("[core] GET_CORE_OPTIONS_VERSION ok=%d v=%u\n",ok,v);
  static struct retro_variable vars[]={{"mupen64plus-rdp-plugin","RDP Plugin; gliden64|angrylion"},{"mupen64plus-rsp-plugin","RSP Plugin; hle|cxd4"},{"mupen64plus-cpucore","CPU; dynamic_recompiler|pure_interpreter"},{NULL,NULL}};
  printf("[core] SET_VARIABLES ok=%d\n", env(16, vars)); printf("[core] BITMASKS ok=%d\n", env(51|0x10000, NULL)); }
void retro_set_video_refresh(video_t cb){video=cb;} void retro_set_audio_sample(asample_t cb){asample=cb;} void retro_set_audio_sample_batch(abatch_t cb){abatch=cb;}
void retro_set_input_poll(poll_t cb){ipoll=cb;} void retro_set_input_state(istate_t cb){istate=cb;}
void retro_set_controller_port_device(unsigned p, unsigned d){printf("[core] port %u device %u\n",p,d);}
void retro_init(void){ struct { log_t log; } l = {0}; env(27,&l); logcb=l.log; printf("[core] log iface=%p\n",(void*)logcb); env(3|0x10000,&clearwaits); printf("[core] clearwaits=%p\n",(void*)clearwaits); env(23,&rum); printf("[core] rumble=%p\n",(void*)rum.set); logcb(1,"Loading %s (%zu bytes) %d%% %s x=%x f=%.2f\n","rom.z64",(size_t)33554432,50,"hello",255,1.5); const char* dir=NULL; bool ok=env(9,&dir); printf("[core] system dir ok=%d %s\n",ok,dir?dir:"(null)"); const char* sd=NULL; env(31,&sd); printf("[core] save dir %s\n", sd?sd:"(null)"); }
void retro_deinit(void){printf("[core] deinit\n");}
void retro_get_system_info(struct retro_system_info*i){memset(i,0,sizeof *i); i->library_name="FakeN64"; i->library_version="1.0"; i->valid_extensions="z64|n64"; i->need_fullpath=false;}
void retro_get_system_av_info(struct retro_system_av_info*i){ i->geometry.base_width=320;i->geometry.base_height=240;i->geometry.max_width=640;i->geometry.max_height=480;i->geometry.aspect_ratio=4.0f/3; i->timing.fps=60.0; i->timing.sample_rate=44100.0;}
bool retro_load_game(const struct retro_game_info*g){ printf("[core] load path=%s size=%zu first=%02x\n", g->path, g->size, g->size? ((const uint8_t*)g->data)[0]:0);
  int fmt=1; printf("[core] pixfmt ok=%d\n", env(10,&fmt)); struct retro_variable v={"mupen64plus-rdp-plugin",NULL}; env(15,&v); printf("[core] rdp=%s\n", v.value?v.value:"(null)");
  struct retro_variable v2={"mupen64plus-rsp-plugin",NULL}; env(15,&v2); printf("[core] rsp=%s\n", v2.value?v2.value:"(null)"); bool hw=env(14,&fmt); printf("[core] hw render ok=%d\n",hw); return true; }
void retro_unload_game(void){printf("[core] unload, sram[0]=%u\n", sram[0]);}
void retro_reset(void){}
void retro_run(void){ clearwaits(1,NULL); rum.set(0,0,100); if(!gstack){ gstack=malloc(8<<20); getcontext(&game_ctx); game_ctx.uc_stack.ss_sp=gstack; game_ctx.uc_stack.ss_size=8<<20; game_ctx.uc_link=&main_ctx; makecontext(&game_ctx, game, 0);} swapcontext(&main_ctx,&game_ctx); ipoll(); int16_t mask=istate(0,1,0,256); int16_t lx=istate(0,5,0,0), ry=istate(0,5,1,1);
  for(int y=0;y<240;y++)for(int x=0;x<320;x++) fb[y*320+x]=0x00FF0000|((x&0xFF)<<8)|(y&0xFF);
  video(fb,320,240,320*4); for(int i=0;i<735*2;i++) audio[i]=(int16_t)(i*40); abatch(audio,735); asample(1,2); sram[0]++; sram[1]=(uint8_t)(gmask>>8); sram[2]=(uint8_t)(lx>>8); sram[3]=(uint8_t)(ry>>8);
  if(frame%30==0) printf("[core] frame %d mask=%d lx=%d ry=%d\n", frame, mask, lx, ry); frame++; }
size_t retro_get_memory_size(unsigned id){ return id==0? sizeof sram:0;} void* retro_get_memory_data(unsigned id){ return id==0? sram:NULL;}
