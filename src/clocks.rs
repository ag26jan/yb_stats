use chrono::{DateTime, Local};
use std::{sync::mpsc::channel, time::Instant};
use serde_derive::{Serialize,Deserialize};
//use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use log::*;
use soup::prelude::*;
use anyhow::Result;
use crate::isleader::AllStoredIsLeader;
use crate::utility::{scan_host_port, http_get};
use crate::snapshot::save_snapshot;

#[derive(Debug)]
pub struct Clocks {
    pub server: String,
    pub time_since_heartbeat: String,
    pub status_uptime: String,
    pub physical_time_utc: String,
    pub hybrid_time_utc: String,
    pub heartbeat_rtt: String,
    pub cloud: String,
    pub region: String,
    pub zone: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StoredClocks {
    pub hostname_port: String,
    pub timestamp: DateTime<Local>,
    pub server: String,
    pub time_since_heartbeat: String,
    pub status_uptime: String,
    pub physical_time_utc: String,
    pub hybrid_time_utc: String,
    pub heartbeat_rtt: String,
    pub cloud: String,
    pub region: String,
    pub zone: String,
}

#[derive(Debug, Default)]
pub struct AllStoredClocks {
    pub stored_clocks: Vec<StoredClocks>
}

impl AllStoredClocks {
    pub async fn perform_snapshot(
        hosts: &Vec<&str>,
        ports: &Vec<&str>,
        snapshot_number: i32,
        parallel: usize ,
    ) -> Result<()>
    {
        info!("begin snapshot");
        let timer = Instant::now();

        let allstoredclocks = AllStoredClocks::read_clocks(hosts, ports, parallel).await?;
        save_snapshot(snapshot_number, "clocks", allstoredclocks.stored_clocks)?;

        info!("end snapshot: {:?}", timer.elapsed());

        Ok(())
    }
    pub fn new() -> Self { Default::default() }
    pub async fn read_clocks (
        hosts: &Vec<&str>,
        ports: &Vec<&str>,
        parallel: usize
    ) -> Result<AllStoredClocks>
    {
        info!("begin parallel http read");
        let timer = Instant::now();

        let pool = rayon::ThreadPoolBuilder::new().num_threads(parallel).build().unwrap();
        let (tx, rx) = channel();

        pool.scope(move |s| {
            for host in hosts {
                for port in ports {
                    let tx = tx.clone();
                    s.spawn(move |_| {
                        let detail_snapshot_time = Local::now();
                        let clocks = AllStoredClocks::read_http(host, port);
                        tx.send((format!("{}:{}", host, port), detail_snapshot_time, clocks)).expect("error sending data via tx (clocks)");
                    });
                }
            }
        });

        info!("end parallel http read {:?}", timer.elapsed());

        let mut allstoredclocks = AllStoredClocks::new();

        for (hostname_port, detail_snapshot_time, clocks) in rx {
            for clock in clocks {
                allstoredclocks.stored_clocks.push(StoredClocks {
                    hostname_port: hostname_port.to_string(),
                    timestamp: detail_snapshot_time,
                    server: clock.server.to_string(),
                    time_since_heartbeat: clock.time_since_heartbeat.to_string(),
                    status_uptime: clock.status_uptime.to_string(),
                    physical_time_utc: clock.physical_time_utc.to_string(),
                    hybrid_time_utc: clock.hybrid_time_utc.to_string(),
                    heartbeat_rtt: clock.heartbeat_rtt.to_string(),
                    cloud: clock.cloud.to_string(),
                    region: clock.region.to_string(),
                    zone: clock.zone.to_string(),
                });
            }
        }
        Ok(allstoredclocks)
    }
    fn read_http(
        host: &str,
        port: &str,
    ) -> Vec<Clocks>
    {
        let data_from_http = if scan_host_port(host, port) {
            http_get(host, port, "tablet-server-clocks?raw")
        } else {
            String::new()
        };
        AllStoredClocks::parse_clocks(data_from_http)
    }
    fn parse_clocks(
        http_data: String,
    ) -> Vec<Clocks>
    {
        let mut clocks: Vec<Clocks> = Vec::new();
        if let Some(table) = AllStoredClocks::find_table(&http_data)
        {
            let (headers, rows) = table;

            let try_find_header = |target| headers.iter().position(|h| h == target);

            let server_pos = try_find_header("Server");
            let time_since_heartbeat_pos = try_find_header("Time since <br>heartbeat");
            let status_uptime_pos = try_find_header("Status &amp; Uptime");
            let physical_time_utc_pos = try_find_header("Physical Time (UTC)");
            let hybrid_time_utc_pos = try_find_header("Hybrid Time (UTC)");
            let heartbeat_rtt_pos = try_find_header("Heartbeat RTT");
            let cloud_pos = try_find_header("Cloud");
            let region_pos = try_find_header("Region");
            let zone_pos = try_find_header("Zone");

            let take_or_missing = |row: &mut [String], pos: Option<usize>|
                match pos.and_then(|pos| row.get_mut(pos))
                {
                    Some(value) => std::mem::take(value),
                    None => "<Missing>".to_string(),
                };

            //let mut stack_from_table = String::from("Initial value: this should not be visible");
            for mut row in rows
            {
                // this is a way to remove some html from the result.
                // not sure if this is the best way, but it fits the purpose.
                let parse = Soup::new(&take_or_missing(&mut row, server_pos));

                clocks.push(Clocks {
                    //server: take_or_missing(&mut row, server_pos),
                    server: parse.text(),
                    time_since_heartbeat: take_or_missing(&mut row, time_since_heartbeat_pos),
                    status_uptime: take_or_missing(&mut row, status_uptime_pos),
                    physical_time_utc: take_or_missing(&mut row, physical_time_utc_pos),
                    hybrid_time_utc: take_or_missing(&mut row, hybrid_time_utc_pos),
                    heartbeat_rtt: take_or_missing(&mut row, heartbeat_rtt_pos),
                    cloud: take_or_missing(&mut row, cloud_pos),
                    region: take_or_missing(&mut row, region_pos),
                    zone: take_or_missing(&mut row, zone_pos),
                });
            }
        }
        clocks
    }
    fn find_table(http_data: &str) -> Option<(Vec<String>, Vec<Vec<String>>)>
    {
        let css = |selector| Selector::parse(selector).unwrap();
        let get_cells = |row: ElementRef, selector| {
            row.select(&css(selector))
                .map(|cell| cell.inner_html().trim().to_string())
                .collect()
        };
        let html = Html::parse_fragment(http_data);
        let table = html.select(&css("table")).next()?;
        let tr = css("tr");
        let mut rows = table.select(&tr);
        let headers = get_cells(rows.next()?, "th");
        let rows: Vec<_> = rows.map(|row| get_cells(row, "td")).collect();
        Some((headers, rows))
    }
    pub fn print(
        &self,
        snapshot_number: &String,
        details_enable: &bool,
    ) -> Result<()>
    {
        info!("print tablet server clocks");

        let leader_hostname = AllStoredIsLeader::return_leader_snapshot(snapshot_number)?;

        for row in &self.stored_clocks {
            if row.hostname_port == leader_hostname
                && !*details_enable
            {
                println!("{} {} {} {} {} {} {} {} {}", row.server, row.time_since_heartbeat, row.status_uptime, row.physical_time_utc, row.hybrid_time_utc, row.heartbeat_rtt, row.cloud, row.region, row.zone);
            }
            if *details_enable
            {
                println!("{}: {} {} {} {} {} {} {} {} {}", row.hostname_port, row.server, row.time_since_heartbeat, row.status_uptime, row.physical_time_utc, row.hybrid_time_utc, row.heartbeat_rtt, row.cloud, row.region, row.zone);
            }
        }
        Ok(())
    }
    pub async fn print_adhoc(
        &self,
        details_enable: &bool,
        hosts: &Vec<&str>,
        ports: &Vec<&str>,
        parallel: usize,
    ) -> Result<()>
    {
        info!("print adhoc tablet servers clocks");

        let leader_hostname = AllStoredIsLeader::return_leader_http(hosts, ports, parallel).await;

        for row in &self.stored_clocks {
            if row.hostname_port == leader_hostname
                && !*details_enable
            {
                println!("{} {} {} {} {} {} {} {} {}", row.server, row.time_since_heartbeat, row.status_uptime, row.physical_time_utc, row.hybrid_time_utc, row.heartbeat_rtt, row.cloud, row.region, row.zone);
            }
            if *details_enable
            {
                println!("{} {} {} {} {} {} {} {} {} {}", row.hostname_port, row.server, row.time_since_heartbeat, row.status_uptime, row.physical_time_utc, row.hybrid_time_utc, row.heartbeat_rtt, row.cloud, row.region, row.zone);
            }
        }
        Ok(())
    }
    pub async fn print_adhoc_latency(
        &self,
        details_enable: &bool,
        hosts: &Vec<&str>,
        ports: &Vec<&str>,
        parallel: usize,
    ) -> Result<()>
    {
        info!("print adhoc tablet servers clocks latency");

        let leader_hostname = AllStoredIsLeader::return_leader_http(hosts, ports, parallel).await;

        for row in &self.stored_clocks {
            if row.hostname_port == leader_hostname
                && !*details_enable
            {
                println!("{} -> {}: {} RTT ({} {} {})", leader_hostname.clone(), row.server.split_whitespace().next().unwrap_or_default(), row.heartbeat_rtt, row.cloud, row.region, row.zone);
            }
            if *details_enable
            {
                println!("{} {} -> {}: {} RTT ({} {} {})", row.hostname_port, leader_hostname.clone(), row.server.split_whitespace().next().unwrap_or_default(), row.heartbeat_rtt, row.cloud, row.region, row.zone);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_parse_threads_data() {
        // This is what /threadz?group=all returns.
        let threads = r#"<!DOCTYPE html><html>  <head>    <title>YugabyteDB</title>    <link rel='shortcut icon' href='/favicon.ico'>    <link href='/bootstrap/css/bootstrap.min.css' rel='stylesheet' media='screen' />    <link href='/bootstrap/css/bootstrap-theme.min.css' rel='stylesheet' media='screen' />    <link href='/font-awesome/css/font-awesome.min.css' rel='stylesheet' media='screen' />    <link href='/yb.css' rel='stylesheet' media='screen' />  </head>
<body>
  <nav class="navbar navbar-fixed-top navbar-inverse sidebar-wrapper" role="navigation">    <ul class="nav sidebar-nav">      <li><a href='/'><img src='/logo.png' alt='YugabyteDB' class='nav-logo' /></a></li>
<li class='nav-item'><a href='/'><div><i class='fa fa-home'aria-hidden='true'></i></div>Home</a></li>
<li class='nav-item'><a href='/tables'><div><i class='fa fa-table'aria-hidden='true'></i></div>Tables</a></li>
<li class='nav-item'><a href='/tablet-servers'><div><i class='fa fa-server'aria-hidden='true'></i></div>Tablet Servers</a></li>
<li class='nav-item'><a href='/utilz'><div><i class='fa fa-wrench'aria-hidden='true'></i></div>Utilities</a></li>
    </ul>  </nav>

    <div class='yb-main container-fluid'><h2>Thread Group: all</h2>
<h3>All Threads : </h3><table class='table table-hover table-border'><tr><th>Thread name</th><th>Cumulative User CPU(s)</th><th>Cumulative Kernel CPU(s)</th><th>Cumulative IO-wait(s)</th></tr><tr><td>Master_reactorx-6127</td><td>2.960s</td><td>0.000s</td><td>0.000s</td><td rowspan="1"><pre>    @     0x7f035af7a9f2  __GI_epoll_wait
    @     0x7f035db7fbf7  epoll_poll
    @     0x7f035db7ac5d  ev_run
    @     0x7f035e02f07b  yb::rpc::Reactor::RunThread()
    @     0x7f035de5f1d4  yb::Thread::SuperviseThread()
    @     0x7f035b83e693  start_thread
    @     0x7f035af7a41c  __clone

Total number of threads: 1</pre></td></tr>
<tr><td>acceptorxxxxxxx-6136</td><td>0.000s</td><td>0.000s</td><td>0.000s</td><td rowspan="1"><pre>    @     0x7f035af7a9f2  __GI_epoll_wait
    @     0x7f035db7fbf7  epoll_poll
    @     0x7f035db7ac5d  ev_run
    @     0x7f035dff41d6  yb::rpc::Acceptor::RunThread()
    @     0x7f035de5f1d4  yb::Thread::SuperviseThread()
    @     0x7f035b83e693  start_thread
    @     0x7f035af7a41c  __clone

Total number of threads: 1</pre></td></tr>
<tr><td>bgtasksxxxxxxxx-6435</td><td>0.630s</td><td>0.000s</td><td>0.000s</td><td rowspan="1"><pre>    @     0x7f035b8433b7  __pthread_cond_timedwait
    @     0x7f035dd52946  yb::ConditionVariable::TimedWait()
    @     0x7f03610c21f0  yb::master::CatalogManagerBgTasks::Wait()
    @     0x7f03610c255b  yb::master::CatalogManagerBgTasks::Run()
    @     0x7f035de5f1d4  yb::Thread::SuperviseThread()
    @     0x7f035b83e693  start_thread
    @     0x7f035af7a41c  __clone

Total number of threads: 1</pre></td></tr>
<tr><td>iotp_Master_3xx-6126</td><td>0.160s</td><td>0.000s</td><td>0.000s</td><td rowspan="3"><pre>    @     0x7f035b84300c  __pthread_cond_wait
    @     0x7f035e00c927  yb::rpc::IoThreadPool::Impl::Execute()
    @     0x7f035de5f1d4  yb::Thread::SuperviseThread()
    @     0x7f035b83e693  start_thread
    @     0x7f035af7a41c  __clone

Total number of threads: 3</pre></td></tr>
<tr><td>iotp_Master_2xx-6125</td><td>0.230s</td><td>0.000s</td><td>0.000s</td></tr>
<tr><td>iotp_Master_0xx-6123</td><td>0.200s</td><td>0.000s</td><td>0.000s</td></tr>
<tr><td>iotp_Master_1xx-6124</td><td>0.220s</td><td>0.000s</td><td>0.000s</td><td rowspan="1"><pre>    @     0x7f035af7a9f2  __GI_epoll_wait
    @     0x7f035e00c9c6  yb::rpc::IoThreadPool::Impl::Execute()
    @     0x7f035de5f1d4  yb::Thread::SuperviseThread()
    @     0x7f035b83e693  start_thread
    @     0x7f035af7a41c  __clone

Total number of threads: 1</pre></td></tr>
<tr><td>iotp_call_home_0-6437</td><td>0.000s</td><td>0.000s</td><td>0.000s</td><td rowspan="1"><pre>    @     0x7f035af7a9f2  __GI_epoll_wait
    @     0x7f035e00c9c6  yb::rpc::IoThreadPool::Impl::Execute()
    @     0x7f035de5f1d4  yb::Thread::SuperviseThread()
    @     0x7f035b83e693  start_thread
    @     0x7f035af7a41c  __clone

Total number of threads: 1</pre></td></tr>
<tr><td>maintenance_scheduler-6130</td><td>1.370s</td><td>0.000s</td><td>0.000s</td><td rowspan="1"><pre>    @     0x7f035b8433b7  __pthread_cond_timedwait
    @     0x7f035c0217ea  std::__1::condition_variable::__do_timed_wait()
    @     0x7f0360ce4cd9  yb::MaintenanceManager::RunSchedulerThread()
    @     0x7f035de5f1d4  yb::Thread::SuperviseThread()
    @     0x7f035b83e693  start_thread
    @     0x7f035af7a41c  __clone

Total number of threads: 1</pre></td></tr>
<tr><td>rb-session-expx-6134</td><td>0.010s</td><td>0.000s</td><td>0.000s</td><td rowspan="1"><pre>    @     0x7f035b8433b7  __pthread_cond_timedwait
    @     0x7f035dd5287c  yb::ConditionVariable::WaitUntil()
    @     0x7f035dd52c46  yb::CountDownLatch::WaitFor()
    @     0x7f03617dbae3  yb::tserver::RemoteBootstrapServiceImpl::EndExpiredSessions()
    @     0x7f035de5f1d4  yb::Thread::SuperviseThread()
    @     0x7f035b83e693  start_thread
    @     0x7f035af7a41c  __clone

Total number of threads: 1</pre></td></tr>
<tr><td>rpc_tp_Master_16-6449</td><td>0.000s</td><td>0.000s</td><td>0.000s</td><td rowspan="18"><pre>    @     0x7f035b84300c  __pthread_cond_wait
    @     0x7f035c021751  std::__1::condition_variable::wait()
    @     0x7f035e06d1ce  yb::rpc::(anonymous namespace)::Worker::Execute()
    @     0x7f035de5f1d4  yb::Thread::SuperviseThread()
    @     0x7f035b83e693  start_thread
    @     0x7f035af7a41c  __clone

Total number of threads: 18</pre></td></tr>
<tr><td>rpc_tp_Master_15-6448</td><td>0.000s</td><td>0.000s</td><td>0.000s</td></tr>
<tr><td>rpc_tp_Master_14-6447</td><td>0.000s</td><td>0.000s</td><td>0.000s</td></tr>
<tr><td>rpc_tp_Master_13-6446</td><td>0.000s</td><td>0.000s</td><td>0.000s</td></tr>
<tr><td>rpc_tp_Master_12-6445</td><td>0.000s</td><td>0.000s</td><td>0.000s</td></tr>
<tr><td>rpc_tp_Master_11-6444</td><td>0.000s</td><td>0.000s</td><td>0.000s</td></tr>
<tr><td>rpc_tp_Master_10-6443</td><td>0.000s</td><td>0.000s</td><td>0.000s</td></tr>
<tr><td>rpc_tp_Master_9-6442</td><td>0.000s</td><td>0.000s</td><td>0.000s</td></tr>
<tr><td>rpc_tp_Master_5-6438</td><td>0.000s</td><td>0.000s</td><td>0.000s</td></tr>
<tr><td>rpc_tp_Master_7-6440</td><td>0.000s</td><td>0.000s</td><td>0.000s</td></tr>
<tr><td>rpc_tp_Master_4-6390</td><td>0.020s</td><td>0.000s</td><td>0.000s</td></tr>
<tr><td>rpc_tp_Master_8-6441</td><td>0.000s</td><td>0.000s</td><td>0.000s</td></tr>
<tr><td>rpc_tp_Master_3-6381</td><td>0.020s</td><td>0.000s</td><td>0.000s</td></tr>
<tr><td>rpc_tp_Master-high-pri_0-6140</td><td>2.990s</td><td>0.000s</td><td>0.000s</td></tr>
<tr><td>rpc_tp_Master_1-6139</td><td>0.020s</td><td>0.000s</td><td>0.000s</td></tr>
<tr><td>rpc_tp_Master_6-6439</td><td>0.000s</td><td>0.000s</td><td>0.000s</td></tr>
<tr><td>rpc_tp_Master_0-6137</td><td>0.020s</td><td>0.000s</td><td>0.000s</td></tr>
<tr><td>rpc_tp_Master_2-6375</td><td>0.060s</td><td>0.000s</td><td>0.000s</td></tr>
<tr><td>flush scheduler bgtask-6434</td><td>0.000s</td><td>0.000s</td><td>0.000s</td><td rowspan="1"><pre>    @     0x7f035b84300c  __pthread_cond_wait
    @     0x7f035c021751  std::__1::condition_variable::wait()
    @     0x7f035dd475ba  yb::BackgroundTask::WaitForJob()
    @     0x7f035dd47393  yb::BackgroundTask::Run()
    @     0x7f035de5f1d4  yb::Thread::SuperviseThread()
    @     0x7f035b83e693  start_thread
    @     0x7f035af7a41c  __clone

Total number of threads: 1</pre></td></tr>
<tr><td>MaintenanceMgr [worker]-6122</td><td>0.000s</td><td>0.000s</td><td>0.000s</td><td rowspan="4"><pre>    @     0x7f035b84300c  __pthread_cond_wait
    @     0x7f035de63bf9  yb::ThreadPool::DispatchThread()
    @     0x7f035de5f1d4  yb::Thread::SuperviseThread()
    @     0x7f035b83e693  start_thread
    @     0x7f035af7a41c  __clone

Total number of threads: 4</pre></td></tr>
<tr><td>log-alloc [worker]-6121</td><td>0.000s</td><td>0.000s</td><td>0.000s</td></tr>
<tr><td>append [worker]-6120</td><td>0.090s</td><td>0.000s</td><td>0.000s</td></tr>
<tr><td>prepare [worker]-6119</td><td>0.100s</td><td>0.000s</td><td>0.000s</td></tr>
</table><div class='yb-bottom-spacer'></div></div>
<footer class='footer'><div class='yb-footer container text-muted'><pre class='message'><i class="fa-lg fa fa-gift" aria-hidden="true"></i> Congratulations on installing YugabyteDB. We'd like to welcome you to the community with a free t-shirt and pack of stickers! Please claim your reward here: <a href='https://www.yugabyte.com/community-rewards/'>https://www.yugabyte.com/community-rewards/</a></pre><pre>version 2.13.0.0 build 42 revision cd3c1a4bb1cca183be824851f8158ebbffd1d3d8 build_type RELEASE built at 06 Mar 2022 03:13:49 UTC
server uuid 4ce571a18f8c4a9a8b35246222d12025 local time 2022-03-16 12:33:37.634419</pre></div></footer></body></html>"#.to_string();
        let result = parse_threads(threads);
        // this results in 33 Threads
        assert_eq!(result.len(), 33);
        // and the thread name is Master_reactorx-6127
        // these are all the fields, for completeness sake
        assert_eq!(result[0].thread_name, "Master_reactorx-6127");
        assert_eq!(result[0].cumulative_user_cpu_s, "2.960s");
        assert_eq!(result[0].cumulative_kernel_cpu_s, "0.000s");
        assert_eq!(result[0].cumulative_iowait_cpu_s, "0.000s");
        //assert_eq!(result[0].stack, "<pre>    @     0x7f035af7a9f2  __GI_epoll_wait\n    @     0x7f035db7fbf7  epoll_poll\n    @     0x7f035db7ac5d  ev_run\n    @     0x7f035e02f07b  yb::rpc::Reactor::RunThread()\n    @     0x7f035de5f1d4  yb::Thread::SuperviseThread()\n    @     0x7f035b83e693  start_thread\n    @     0x7f035af7a41c  __clone\n\nTotal number of threads: 1</pre>");
        assert_eq!(result[0].stack, "__clone;start_thread;yb::Thread::SuperviseThread();yb::rpc::Reactor::RunThread();ev_run;epoll_poll;__GI_epoll_wait");
    }

    use crate::utility;

    #[test]
    fn integration_parse_threadsdata_master() {
        let mut stored_threadsdata: Vec<StoredThreads> = Vec::new();
        let detail_snapshot_time = Local::now();
        let hostname = utility::get_hostname_master();
        let port = utility::get_port_master();

        let data_parsed_from_json = read_threads(hostname.as_str(), port.as_str());
        add_to_threads_vector(data_parsed_from_json, format!("{}:{}", hostname, port).as_str(), detail_snapshot_time, &mut stored_threadsdata);
        // each daemon should return one row.
        assert!(stored_threadsdata.len() > 1);
    }
    #[test]
    fn integration_parse_threadsdata_tserver() {
        let mut stored_threadsdata: Vec<StoredThreads> = Vec::new();
        let detail_snapshot_time = Local::now();
        let hostname = utility::get_hostname_tserver();
        let port = utility::get_port_tserver();

        let data_parsed_from_json = read_threads(hostname.as_str(), port.as_str());
        add_to_threads_vector(data_parsed_from_json, format!("{}:{}", hostname, port).as_str(), detail_snapshot_time, &mut stored_threadsdata);
        // each daemon should return one row.
        assert!(stored_threadsdata.len() > 1);
    }

}