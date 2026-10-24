#include "molite.h"
#include <stdio.h>
#include <string.h>
int main(int argc,char **argv){
 if(argc!=2)return 10;
 molite_engine engine=0;molite_task task=0;
 const char *request="{\"version\":1,\"op\":\"resource.list\",\"kind\":\"scene\"}";
 if(molite_abi_version()!=1)return 11;
 if(molite_open((const uint8_t*)argv[1],strlen(argv[1]),&engine)!=MOLITE_OK)return 12;
 if(molite_submit(engine,(const uint8_t*)request,strlen(request),&task)!=MOLITE_OK)return 13;
 if(molite_close(engine)!=MOLITE_BUSY)return 14;
 int finished=0;
 for(int i=0;i<30&&!finished;i++){
  molite_buffer buffer={0};int status=molite_task_next(task,1000,&buffer);
  if(status==MOLITE_EMPTY)continue;if(status!=MOLITE_OK)return 15;
  char text[4096];if(buffer.len>=sizeof(text))return 16;
  memcpy(text,buffer.data,buffer.len);text[buffer.len]=0;puts(text);
  finished=strstr(text,"\"completed\"")!=NULL;molite_buffer_free(buffer);
 }
 if(!finished)return 17;
 if(molite_task_release(task)!=MOLITE_OK)return 18;
 if(molite_close(engine)!=MOLITE_OK)return 19;
 if(molite_close(engine)!=MOLITE_INVALID)return 20;
 return 0;
}
